use std::path::PathBuf;

use async_tempfile::TempFile;
use axum::{
    body::Body,
    extract::{Multipart, Path as ExtractPath, Query, State},
    http::{header, Response, StatusCode},
};
use chrono::{DateTime, Utc};
use md5::Md5;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
};
use tokio_tar::Builder;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    database::{AppState, FileInfo, FileList, TimePoint},
    response::{ApiResponse, ApiResult, Error, Result},
};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(upload_file))
        .routes(routes!(get_filesystem_at))
        .with_state(state)
}

#[derive(Deserialize, ToSchema)]
struct RequestTimePoint {
    timestamp: Option<String>,
    delta: Option<String>,
}

impl TryFrom<RequestTimePoint> for TimePoint {
    type Error = Error;

    fn try_from(value: RequestTimePoint) -> std::result::Result<Self, Self::Error> {
        Ok(match (value.timestamp, value.delta) {
            (Some(timestamp), _) => {
                TimePoint::Absolute(DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc))
            }
            (None, Some(delta)) => todo!(),
            _ => TimePoint::Absolute(Utc::now()),
        })
    }
}

#[utoipa::path(
    get,
    path = "/{*path}",
    tag = "files",
    responses(
        (status = 200, description = "The filepaths were successfully returned")
    )
)]
async fn get_filesystem_at(
    State(app): State<AppState>,
    ExtractPath(path): ExtractPath<String>,
    Query(time_point): Query<RequestTimePoint>,
) -> Result<Response<Body>> {
    let files = app.get_file_paths(&path, time_point.try_into()?).await?;

    let body = match files {
        FileList::None => return Err(Error::FileNotFound(PathBuf::from(path))),
        FileList::SingleFile(path) =>  Body::from_stream(ReaderStream::new(File::open(path).await?)),
        FileList::MultipleFile(files) => {
            let buffer = TempFile::new().await?;
            let mut builder = Builder::new(buffer);

            for (absolute_local_path, archive_path) in files {
                let mut file = File::open(absolute_local_path).await?;
                builder.append_file(archive_path, &mut file).await?;
            }

            let file = builder.into_inner().await?.open_ro().await?;
            Body::from_stream(ReaderStream::new(file))
        }
    };

    let response = Response::builder()
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
    path = "/{*path}",
    tag = "files",
    responses(
        (status = 201, description = "The file was successfully uploaded")
    )
)]
async fn upload_file(
    State(state): State<AppState>,
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

    state
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
