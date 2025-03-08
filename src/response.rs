use std::io;

use axum::{
    Json,
    extract::multipart::MultipartError,
    http::{self, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::path::StoragePath;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io related error")]
    ServerIo(#[from] io::Error),

    #[error("HTTP related error")]
    Http(#[from] http::Error),

    #[error("SQL related error")]
    Sql(#[from] sqlx::Error),

    #[error("tmpfs related error")]
    TempFile(#[from] async_tempfile::Error),

    #[error("invalid timestamp format")]
    TimestampParseError(#[from] chrono::ParseError),

    #[error("multipart related error")]
    Multipart(#[from] MultipartError),

    #[error("multipart missing a field")]
    MultipartMissingField(String),

    #[error("the given path is not a valid file path")]
    InvalidFilePath(StoragePath),

    #[error("no file at the given path")]
    FileNotFound(StoragePath),

    #[error("could not build the archive")]
    ArchiveGenerationFailed,

    #[error("unknown error: {0}")]
    Unknown(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Serialize, ToSchema)]
#[repr(u8)]
enum ApiErrorCode {
    ServerSideIoFailure = 0,
    ServerSideHttpFailure,
    ServerSideSqlFailure,
    ServerSideTempFileFailure,
    InvalidTimestamp,
    MalformedMultipart,
    MultipartMissingField,
    InvalidFilePath,
    FileNotFound,
    ArchiveGenerationFailed,
    UnknownError = u8::MAX,
}

#[derive(Serialize, ToSchema)]
pub struct ApiError {
    api_error_code: ApiErrorCode,
    error_message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status")]
enum ApiResponseData<T> {
    Success(T),
    Failure(ApiError),
}

pub struct ApiResponse<T> {
    status_code: StatusCode,
    data: ApiResponseData<T>,
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize + ToSchema,
{
    fn into_response(self) -> axum::response::Response {
        (self.status_code, Json(self.data)).into_response()
    }
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        ApiResponse::success_with_status_code(StatusCode::OK, data)
    }

    pub fn success_with_status_code(status_code: StatusCode, data: T) -> Self {
        Self {
            status_code,
            data: ApiResponseData::Success(data),
        }
    }
}

pub type ApiResult<T> = Result<ApiResponse<T>>;

impl<T> From<Error> for ApiResponse<T> {
    fn from(error: Error) -> Self {
        let (status_code, error_code, error_message) = match error {
            Error::ServerIo(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::ServerSideIoFailure,
                format!("a server-side IO error occurred: {}", error),
            ),
            Error::Http(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::ServerSideHttpFailure,
                format!("a server-side HTTP error occurred: {}", error),
            ),
            Error::Sql(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::ServerSideSqlFailure,
                format!("a server-side SQL error occurred: {}", error),
            ),
            Error::TempFile(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::ServerSideTempFileFailure,
                format!("a server-side temp file error occurred: {}", error),
            ),
            Error::TimestampParseError(error) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::InvalidTimestamp,
                format!(
                    "the given timestamp is not a valid rfc3339 timestamp: {}",
                    error
                ),
            ),
            Error::Multipart(error) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::MalformedMultipart,
                error.to_string(),
            ),
            Error::MultipartMissingField(field) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::MultipartMissingField,
                format!("the field '{}' is missing from the multipart", field),
            ),
            Error::InvalidFilePath(path) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::InvalidFilePath,
                format!("the path '{}' does not point to a file", path.to_str()),
            ),
            Error::FileNotFound(path) => (
                StatusCode::NOT_FOUND,
                ApiErrorCode::FileNotFound,
                format!("the path '{}' does not point to a live file", path.to_str()),
            ),
            Error::ArchiveGenerationFailed => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::ArchiveGenerationFailed,
                String::from("could not generate the archive"),
            ),
            Error::Unknown(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::UnknownError,
                format!("unknown error: {}", error),
            ),
        };

        ApiResponse {
            status_code,
            data: ApiResponseData::Failure(ApiError {
                api_error_code: error_code,
                error_message,
            }),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        ApiResponse::<()>::from(self).into_response()
    }
}
