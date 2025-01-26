use std::{io, path::PathBuf};

use axum::{
    extract::{multipart::MultipartError, Multipart, State},
    http::StatusCode,
};
use tokio::{fs::remove_file, fs::File, io::AsyncWriteExt};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    database::AppState,
    response::{ApiErrorCode, ApiResponse},
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
async fn upload_file(State(state): State<AppState>, multipart: Multipart) -> ApiResponse<()> {
    // TODO generate unique path, maybe use uuid to ensure uniqueness.
    let path_temp_file = "/tmp/test";

    let parsing_error = {
        let mut temp_file = match File::create(&path_temp_file).await {
            Ok(file) => file,
            Err(error) => return ApiResponse::failure(ApiErrorCode::ServerIO, &error.to_string()),
        };

        let parsed_result = parse_multipart(&mut temp_file, multipart).await;

        if let Ok(parsed_path) = &parsed_result {
            // TODO put the file in the storage using the given state and copy tempfile.
            println!("parsed_path={:#?}", parsed_path);
        }

        parsed_result.err()
    };

    if let Err(error) = remove_file(path_temp_file).await {
        // TODO: we don't really want to throw an error if removing the temp
        // file failed, maybe raise a warning on the server side?
        return ApiResponse::failure(ApiErrorCode::ServerIO, &error.to_string());
    }

    match parsing_error {
        Some(error) => error.into(),
        None => ApiResponse::success_with_status_code(StatusCode::CREATED, ()),
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParsingError {
    #[error("multipart related error")]
    Multipart(#[from] MultipartError),

    #[error("io related error")]
    Io(#[from] io::Error),

    #[error("path is missing from the multipart")]
    MissingPath,
}

impl<T> From<ParsingError> for ApiResponse<T> {
    fn from(value: ParsingError) -> Self {
        let (api_error_code, error_message) = match value {
            ParsingError::Io(error) => (ApiErrorCode::ServerIO, error.to_string()),
            ParsingError::Multipart(error) => {
                (ApiErrorCode::InvalidMultipartFile, error.to_string())
            }
            ParsingError::MissingPath => (
                ApiErrorCode::InvalidMultipartFile,
                "expected the multipart data to contains a field named 'path'".into(),
            ),
        };

        Self::failure(api_error_code, &error_message)
    }
}

async fn parse_multipart(
    temp_file: &mut File,
    mut multipart: Multipart,
) -> Result<PathBuf, ParsingError> {
    let mut parsed_path = None;

    while let Some(mut field) = multipart.next_field().await? {
        match field.name() {
            Some("path") => {
                parsed_path = Some(PathBuf::from(field.text().await?));
            }
            Some("file") => {
                while let Some(chunk) = field.chunk().await? {
                    temp_file.write_all(&chunk).await?;
                }
            }
            _ => (),
        }
    }

    parsed_path.ok_or(ParsingError::MissingPath)
}
