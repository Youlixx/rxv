use std::{
    io, mem::ManuallyDrop, path::{Path, PathBuf}
};

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
};
use md5::Md5;
use sha2::{Digest, Sha256};
use tokio::{fs::remove_file, fs::File, io::AsyncWriteExt};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    database::{AppState, FileInfo},
    response::{ApiResponse, ApiResult, Error, Result},
};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(upload_file))
        .with_state(state)
}

#[derive(ToSchema)]
#[allow(unused)]
struct UploadFileRequest {
    path: String,
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
    path = "/",
    tag = "files",
    responses(
        (status = 201, description = "The file was successfully uploaded")
    )
)]
async fn upload_file(State(state): State<AppState>, mut multipart: Multipart) -> ApiResult<()> {
    // TODO generate unique path, maybe use uuid to ensure uniqueness.
    let path_temp_file = "/tmp/test";
    let mut temp_file = TempFile::new(path_temp_file).await?;
    // let parsing_results = ParsingResults::parse(&mut temp_file.file, multipart).await?;

    let mut parsed_path = None;

    let mut file_info_builder = FileInfoBuilder::new();
    let mut hasher_md5 = Md5::new();
    let mut hasher_sha256 = Sha256::new();

    while let Some(mut field) = multipart.next_field().await? {
        match field.name() {
            Some("path") => {
                parsed_path = Some(PathBuf::from(field.text().await?));
            }
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
            &parsed_path.ok_or(Error::MultipartMissingField("path".into()))?,
            path_temp_file,
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
