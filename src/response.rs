use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct ApiError {
    api_error_code: u8,
    error_message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status")]
enum ApiResponseData<T: Serialize + ToSchema> {
    Success(T),
    Failure(ApiError),
}

pub struct ApiResponse<T: Serialize + ToSchema> {
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

impl<T> From<Result<(StatusCode, T), (StatusCode, ApiError)>> for ApiResponse<T>
where
    T: Serialize + ToSchema,
{
    fn from(value: Result<(StatusCode, T), (StatusCode, ApiError)>) -> Self {
        match value {
            Ok((status_code, value)) => Self {
                status_code,
                data: ApiResponseData::Success(value),
            },
            Err((status_code, error)) => Self {
                status_code,
                data: ApiResponseData::Failure(error),
            },
        }
    }
}

impl<T> ApiResponse<T>
where
    T: Serialize + ToSchema,
{
    pub fn success(data: T) -> Self {
        ApiResponse::success_with_status_code(StatusCode::OK, data)
    }

    pub fn success_with_status_code(status_code: StatusCode, data: T) -> Self {
        Self {
            status_code,
            data: ApiResponseData::Success(data),
        }
    }

    pub fn failure(status_code: StatusCode, api_error_code: u8, error_message: &str) -> Self {
        Self {
            status_code,
            data: ApiResponseData::Failure(ApiError {
                api_error_code,
                error_message: error_message.to_string(),
            }),
        }
    }
}
