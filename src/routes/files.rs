use std::path::PathBuf;

use async_tempfile::TempFile;
use axum::{
    body::Body,
    extract::{Multipart, Path as ExtractPath, Query, State},
    http::{header, Response, StatusCode},
    routing::get,
};
use chrono::{DateTime, TimeDelta, Utc};
use md5::Md5;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{fs::File, io::AsyncWriteExt};
use tokio_tar::Builder;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    database::{upload::FileInfo, AppState, FileList},
    path::StoragePath,
    response::{ApiResponse, ApiResult, Error, Result},
};

pub fn router(storage: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/files/", get(download_file))
        .routes(routes!(download_file, upload_file, delete_file))
        .with_state(storage)
}

#[derive(Deserialize, ToSchema)]
struct RequestTimePoint {
    timestamp: Option<String>,
    seconds: Option<i64>,
}

impl TryFrom<RequestTimePoint> for DateTime<Utc> {
    type Error = Error;

    fn try_from(value: RequestTimePoint) -> std::result::Result<Self, Self::Error> {
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
async fn download_file(
    State(storage): State<AppState>,
    path: Option<ExtractPath<String>>,
    Query(time_point): Query<RequestTimePoint>,
) -> Result<Response<Body>> {
    let path = match path {
        Some(ExtractPath(path)) => StoragePath::from(path),
        None => StoragePath::default(),
    };

    let files = storage
        .get_file_paths(&path, time_point.try_into()?)
        .await?;

    let mut response_builder = Response::builder();

    let body = match files {
        FileList::None => return Err(Error::FileNotFound(PathBuf::from(path.to_str()))),
        FileList::SingleFile(path) => Body::from_stream(ReaderStream::new(File::open(path).await?)),
        FileList::MultipleFile(files) => {
            let buffer = TempFile::new().await?;
            let mut builder = Builder::new(buffer);

            for (path_file, storage_path) in files {
                let path_archive = path
                    .remove_prefix(&storage_path)
                    .ok_or(Error::ArchiveGenerationFailed)?
                    .to_path_buf();

                let mut file = File::open(path_file).await?;
                builder.append_file(path_archive, &mut file).await?;
            }

            response_builder = response_builder.header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}.tar\"", path.filename()),
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
struct FileInfoBuilder {
    original_name: Option<String>,
    size_in_bytes: Option<i64>,
    hashes: Option<(String, String)>,
}

impl FileInfoBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn original_name(&mut self, original_name: &str) {
        self.original_name = Some(original_name.into());
    }

    fn size_in_bytes(&mut self, size_in_bytes: i64) {
        self.size_in_bytes = Some(size_in_bytes);
    }

    fn hashes(&mut self, hash_md5: String, hash_sha256: String) {
        self.hashes = Some((hash_md5, hash_sha256));
    }

    fn build(self) -> Option<FileInfo> {
        match (self.original_name, self.size_in_bytes, self.hashes) {
            (Some(original_name), Some(size_in_bytes), Some((hash_md5, hash_sha256))) => {
                Some(FileInfo {
                    original_name,
                    size_in_bytes,
                    hash_md5,
                    hash_sha256,
                })
            }
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
async fn upload_file(
    State(storage): State<AppState>,
    ExtractPath(path): ExtractPath<String>,
    mut multipart: Multipart,
) -> ApiResult<()> {
    let mut temp_file = TempFile::new().await?;
    let mut file_info_builder = FileInfoBuilder::new();
    let mut hasher_md5 = Md5::new();
    let mut hasher_sha256 = Sha256::new();

    while let Some(mut field) = multipart.next_field().await? {
        if let Some("file") = field.name() {
            let mut size_in_bytes = 0;

            while let Some(chunk) = field.chunk().await? {
                temp_file.write_all(&chunk).await?;

                size_in_bytes += chunk.len();
                hasher_sha256.update(&chunk);
                hasher_md5.update(chunk);
            }

            file_info_builder.size_in_bytes(size_in_bytes as i64);

            if let Some(original_name) = field.file_name() {
                file_info_builder.original_name(original_name);
            }
        }
    }

    let hash_md5 = hex::encode(hasher_md5.finalize());
    let hash_sha256 = hex::encode(hasher_sha256.finalize());
    file_info_builder.hashes(hash_md5, hash_sha256);

    storage
        .add_new_file_to_storage(
            temp_file.file_path(),
            &path,
            file_info_builder
                .build()
                .ok_or(Error::MultipartMissingField("file".into()))?,
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
    State(storage): State<AppState>,
    ExtractPath(path): ExtractPath<String>,
) -> ApiResult<()> {
    storage
        .delete_file_from_storage(&path)
        .await
        .map(|_| ApiResponse::success(()))
}
