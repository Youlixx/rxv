use std::path::PathBuf;

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
    database::AppState,
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
async fn upload_file(State(state): State<AppState>, multipart: Multipart) -> ApiResult<()> {
    // TODO generate unique path, maybe use uuid to ensure uniqueness.
    let path_temp_file = "/tmp/test";

    let parsing_results = {
        let mut temp_file = File::create(&path_temp_file).await?;
        ParsingResults::parse(&mut temp_file, multipart).await
    };

    if let Ok(parsing_results) = &parsing_results {
        // TODO put the file in the storage using the given state and copy tempfile.
        println!("parsing_results={:#?}", parsing_results);
        state
            .add_new_file_to_storage(
                &parsing_results.path_storage,
                path_temp_file,
                &parsing_results.hash_md5,
                &parsing_results.hash_sha256,
            )
            .await;
    }

    if let Err(error) = remove_file(path_temp_file).await {
        // TODO: we don't really want to throw an error if removing the temp
        // file failed, maybe raise a warning on the server side?
        return Err(Error::ServerIo(error));
    }

    parsing_results.map(|_| ApiResponse::success_with_status_code(StatusCode::CREATED, ()))
}

#[derive(Debug)]
struct ParsingResults {
    path_storage: PathBuf,
    hash_md5: String,
    hash_sha256: String,
}

impl ParsingResults {
    async fn parse(temp_file: &mut File, mut multipart: Multipart) -> Result<Self> {
        let mut parsed_path = None;

        let mut file_was_present = false;
        let mut hasher_md5 = Md5::new();
        let mut hasher_sha256 = Sha256::new();

        while let Some(mut field) = multipart.next_field().await? {
            match field.name() {
                Some("path") => {
                    parsed_path = Some(PathBuf::from(field.text().await?));
                }
                Some("file") => {
                    while let Some(chunk) = field.chunk().await? {
                        temp_file.write_all(&chunk).await?;

                        hasher_sha256.update(&chunk);
                        hasher_md5.update(chunk);
                    }

                    file_was_present = true;
                }
                _ => (),
            }
        }

        if !file_was_present {
            return Err(Error::MultipartMissingField("file".into()));
        }

        let hash_md5 = hex::encode(hasher_md5.finalize());
        let hash_sha256 = hex::encode(hasher_sha256.finalize());

        Ok(Self {
            path_storage: parsed_path.ok_or(Error::MultipartMissingField("path".into()))?,
            hash_md5,
            hash_sha256,
        })
    }
}
