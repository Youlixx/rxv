use async_tempfile::TempFile;
use axum::{
    body::Body,
    extract::{Multipart, Path as ExtractPath, Query, State},
    http::{Response, StatusCode, header},
    routing::get,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{fs::File, io::AsyncWriteExt};
use tokio_tar::Builder;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::database::{
    FileDatabase, error::Error, get_file::FileEntries, save_file::FileMetadata,
    virtual_path::VirtualPath,
};

mod tree;

use super::response::{ApiError, ApiResponse, ApiResult, Result};

const ROOT_ARCHIVE_NAME: &str = "archive";

pub fn router(database: FileDatabase) -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/files/", get(get_file))
        .routes(routes!(get_file, save_file, delete_file))
        .route("/tree/", get(tree::endpoint_tree))
        .routes(routes!(tree::endpoint_tree))
        .routes(routes!(tree::endpoint_metadata))
        .with_state(database)
}

#[derive(Deserialize, ToSchema)]
struct RequestTimestamp {
    timestamp: Option<String>,
    seconds: Option<i64>,
}

impl TryFrom<RequestTimestamp> for DateTime<Utc> {
    type Error = ApiError;

    fn try_from(value: RequestTimestamp) -> std::result::Result<Self, Self::Error> {
        Ok(match (value.timestamp, value.seconds) {
            (Some(timestamp), _) => DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc),
            (None, Some(seconds)) => Utc::now() - TimeDelta::seconds(seconds),
            _ => Utc::now(),
        })
    }
}

#[utoipa::path(
    get,
    path = "/files/{*path}",
    tag = "files",
    responses(
        (status = 200, description = "The filepaths were successfully returned")
    )
)]
async fn get_file(
    State(storage): State<FileDatabase>,
    path: Option<ExtractPath<String>>,
    Query(timestamp): Query<RequestTimestamp>,
) -> Result<Response<Body>> {
    let path = match path {
        Some(ExtractPath(path)) => VirtualPath::from(path),
        None => VirtualPath::default(),
    };

    let files = storage
        .get_file(path.clone(), timestamp.try_into()?)
        .await?;

    let mut response_builder = Response::builder();

    let body = match files {
        FileEntries::None => return Err(Error::VirtualFileNotFound(path).into()),
        FileEntries::SingleFile(entry) => Body::from_stream(ReaderStream::new(
            File::open(entry.path_physical_file).await?,
        )),
        FileEntries::MultipleFiles(entries) => {
            let buffer = TempFile::new().await?;
            let mut builder = Builder::new(buffer);

            for entry in entries {
                let path_archive = &entry.virtual_path.path()[path.path().len()..];
                let mut file = File::open(entry.path_physical_file).await?;
                builder.append_file(path_archive, &mut file).await?;
            }

            response_builder = response_builder.header(
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}.tar\"",
                    path.filename().unwrap_or(ROOT_ARCHIVE_NAME)
                ),
            );

            let file = builder.into_inner().await?.open_ro().await?;
            Body::from_stream(ReaderStream::new(file))
        }
    };

    let response = response_builder
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(body)?;

    Ok(response)
}

#[derive(ToSchema)]
#[allow(unused)]
struct UploadFileRequest {
    #[schema(format = Binary, content_media_type = "application/octet-stream")]
    file: String,
}

#[derive(Default)]
struct FileMetadataBuilder {
    original_file_name: Option<String>,
    size_in_bytes: Option<usize>,
    hash: Option<String>,
}

impl FileMetadataBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn original_name(&mut self, original_name: &str) {
        self.original_file_name = Some(original_name.into());
    }

    fn size_in_bytes(&mut self, size_in_bytes: usize) {
        self.size_in_bytes = Some(size_in_bytes);
    }

    fn hash(&mut self, hash: String) {
        self.hash = Some(hash);
    }

    fn build(self) -> Option<FileMetadata> {
        match (self.original_file_name, self.size_in_bytes, self.hash) {
            (Some(original_file_name), Some(size_in_bytes), Some(hash)) => Some(FileMetadata {
                original_file_name,
                size_in_bytes,
                hash,
            }),
            _ => None,
        }
    }
}

#[utoipa::path(
    post,
    request_body(
        content = inline(UploadFileRequest),
        content_type = "multipart/form-data"
    ),
    path = "/files/{*path}",
    tag = "files",
    responses(
        (status = 201, description = "The file was successfully uploaded")
    )
)]
async fn save_file(
    State(database): State<FileDatabase>,
    ExtractPath(path): ExtractPath<String>,
    mut multipart: Multipart,
) -> ApiResult<()> {
    let mut temp_file = TempFile::new().await?;
    let mut file_info_builder = FileMetadataBuilder::new();
    let mut hasher_sha256 = Sha256::new();

    while let Some(mut field) = multipart.next_field().await? {
        if let Some("file") = field.name() {
            let mut size_in_bytes = 0;

            while let Some(chunk) = field.chunk().await? {
                temp_file.write_all(&chunk).await?;

                size_in_bytes += chunk.len();
                hasher_sha256.update(&chunk);
            }

            file_info_builder.size_in_bytes(size_in_bytes);

            if let Some(original_name) = field.file_name() {
                file_info_builder.original_name(original_name);
            }
        }
    }

    let hash = hex::encode(hasher_sha256.finalize());
    file_info_builder.hash(hash);

    database
        .save_file(
            temp_file.file_path(),
            path,
            Utc::now(),
            file_info_builder
                .build()
                .ok_or(ApiError::MultipartMissingField("file".to_owned()))?,
        )
        .await?;

    Ok(ApiResponse::success_with_status_code(
        StatusCode::CREATED,
        (),
    ))
}

#[utoipa::path(
    delete,
    path = "/files/{*path}",
    tag = "files",
    responses(
        (status = 200, description = "The file or folder got deleted")
    )
)]
async fn delete_file(
    State(database): State<FileDatabase>,
    ExtractPath(path): ExtractPath<String>,
) -> ApiResult<()> {
    database
        .delete_file(path, Utc::now())
        .await
        .map(|_| ApiResponse::success(()))
        .map_err(|err| err.into())
}
