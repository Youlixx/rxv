use std::io;

use axum::{extract::multipart::MultipartError, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use utoipa::ToSchema;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io related error")]
    ServerIo(#[from] io::Error),

    #[error("multipart related error")]
    Multipart(#[from] MultipartError),

    #[error("multipart missing a field")]
    MultipartMissingField(String),

    #[error("unknown error: {0}")]
    Unknown(#[from] Box<dyn std::error::Error + Send + Sync>),
}

mod error_code {
    pub type ErrorCode = u8;
    pub const API_SERVER_SIDE_IO_ERROR: ErrorCode = 0;
    pub const API_MALFORMED_MULTIPART_ERROR: ErrorCode = 1;
    pub const API_MULTIPART_MISSING_FIELD_ERROR: ErrorCode = 2;
    pub const API_UNKNOWN_ERROR: ErrorCode = ErrorCode::MAX;
}

#[derive(Serialize, ToSchema)]
pub struct ApiError {
    api_error_code: error_code::ErrorCode,
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
                error_code::API_SERVER_SIDE_IO_ERROR,
                error.to_string(),
            ),
            Error::Multipart(error) => (
                StatusCode::BAD_REQUEST,
                error_code::API_MALFORMED_MULTIPART_ERROR,
                error.to_string(),
            ),
            Error::MultipartMissingField(field) => (
                StatusCode::BAD_REQUEST,
                error_code::API_MULTIPART_MISSING_FIELD_ERROR,
                format!("the field '{}' is missing from the multipart", field),
            ),
            Error::Unknown(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_code::API_UNKNOWN_ERROR,
                error.to_string()
            )
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
