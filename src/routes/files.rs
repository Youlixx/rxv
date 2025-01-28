use std::{
    io,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
};

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
    fs::{remove_file, File},
    io::AsyncWriteExt,
};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    database::{AppState, FileInfo, TimePoint},
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
    Query(query): Query<RequestTimePoint>,
) -> Result<Response<Body>> {
    // TODO do this properly. have some reflection on who should manage the
    // resource access.
    let time_point: TimePoint = query.try_into()?;

    // TODO this shit's ugly...
    // TODO: remove the expects!
    if path.ends_with("/") {
        let path_file = app.download_folder_from_storage(&path, time_point).await?;

        let response = {
            let file = tokio::fs::File::open(&path_file).await?;

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from_stream(ReaderStream::new(file)))
                .expect("Failed to build response")
        };

        remove_file(path_file).await?;

        Ok(response)
    } else {
        let path_file = app.download_file_from_storage(path, time_point).await?;
        let file = tokio::fs::File::open(path_file).await?;

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from_stream(ReaderStream::new(file)))
            .expect("Failed to build response");

        Ok(response)
    }
}

#[derive(ToSchema)]
#[allow(unused)]
struct UploadFileRequest {
    #[schema(format = Binary, content_media_type = "application/octet-stream")]
    file: String,
}

struct TempFile {
    path: PathBuf,
    file: ManuallyDrop<File>,
}

impl TempFile {
    async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            file: ManuallyDrop::new(File::create(&path).await?),
            path,
        })
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf).await
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.file);
        }

        let path = self.path.clone();
        tokio::spawn(async move {
            // TODO we could print out a warning if this fails.
            remove_file(path)
                .await
                .expect("Failed to delete the temp file.");
        });
    }
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
    // TODO generate unique path, maybe use uuid to ensure uniqueness.
    let path_temp_file = "/tmp/test";
    let mut temp_file = TempFile::new(path_temp_file).await?;

    let mut file_info_builder = FileInfoBuilder::new();
    let mut hasher_md5 = Md5::new();
    let mut hasher_sha256 = Sha256::new();

    while let Some(mut field) = multipart.next_field().await? {
        match field.name() {
            Some("file") => {
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
            _ => (),
        }
    }

    let hash_md5 = hex::encode(hasher_md5.finalize());
    let hash_sha256 = hex::encode(hasher_sha256.finalize());
    file_info_builder.hashes(hash_md5, hash_sha256);

    state
        .add_new_file_to_storage(
            path_temp_file,
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
