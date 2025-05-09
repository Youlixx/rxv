use std::io;

use axum::{
    Json,
    extract::multipart::MultipartError,
    http::{self, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use serde_repr::Serialize_repr;
use utoipa::ToSchema;

use crate::database::error::{Error, InternalError};

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApiError {
    #[error("Database error")]
    Database(#[from] Error),

    #[error("TempFS error")]
    TempFs(#[from] async_tempfile::Error),

    #[error("HTTP related error")]
    Http(#[from] http::Error),

    #[error("Timestamp parsing error")]
    InvalidTimestamp(#[from] chrono::ParseError),

    #[error("Malformed multipart form")]
    MalformedMultipartForm(#[from] MultipartError),

    #[error("Field missing from the multipart form")]
    MultipartMissingField(String),
}

impl From<io::Error> for ApiError {
    fn from(value: io::Error) -> Self {
        ApiError::Database(Error::Internal(InternalError::Io(value)))
    }
}

#[derive(Serialize_repr, ToSchema)]
#[repr(u8)]
enum ApiErrorCode {
    Internal = 0,
    NotAFile,
    FileNotFound,
    InvalidTimestamp,
    MalformedMultipartForm,
    Unknown = u8::MAX,
}

#[derive(Serialize, ToSchema)]
pub struct ApiErrorData {
    api_error_code: ApiErrorCode,
    error_message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status")]
enum ApiResponseData<T> {
    Success(T),
    Failure(ApiErrorData),
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

impl<T> From<ApiError> for ApiResponse<T> {
    fn from(error: ApiError) -> Self {
        let (status_code, error_code, error_message) = match error {
            ApiError::Database(Error::Internal(error)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                format!("An internal server error occurred: {}", error),
            ),
            ApiError::TempFs(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                format!("An internal server error occurred: {}", error),
            ),
            ApiError::Http(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                format!("An internal server error occurred: {}", error),
            ),
            ApiError::Database(Error::NotAVirtualFile(path)) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::NotAFile,
                format!(
                    "The given path should point to a file ({} points to a directory)",
                    path.path()
                ),
            ),
            ApiError::Database(Error::VirtualFileNotFound(path)) => (
                StatusCode::NOT_FOUND,
                ApiErrorCode::FileNotFound,
                format!("File not found: {}", path.path()),
            ),
            ApiError::InvalidTimestamp(error) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::InvalidTimestamp,
                format!("Unable to parse the timestamp: {}", error),
            ),
            ApiError::MalformedMultipartForm(error) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::MalformedMultipartForm,
                format!("Malformed multipart form: {}", error),
            ),
            ApiError::MultipartMissingField(field) => (
                StatusCode::BAD_REQUEST,
                ApiErrorCode::MalformedMultipartForm,
                format!("Missing field '{}' from the multipart form", field),
            ),
            ApiError::Database(Error::Unknown(error)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Unknown,
                format!("Unknown error: {}", error),
            ),
        };

        ApiResponse {
            status_code,
            data: ApiResponseData::Failure(ApiErrorData {
                api_error_code: error_code,
                error_message,
            }),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        ApiResponse::<()>::from(self).into_response()
    }
}
