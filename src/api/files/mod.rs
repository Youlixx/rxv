use axum::routing::{delete, get};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::database::FileDatabase;

mod files;
mod metadata;

use super::response::ApiError;

pub fn router(database: FileDatabase) -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/files/", get(files::endpoint_download_file))
        .routes(routes!(files::endpoint_download_file))
        .routes(routes!(files::endpoint_upload_file))
        .route("/files/", delete(files::endpoint_delete_file))
        .routes(routes!(files::endpoint_delete_file))
        .routes(routes!(files::endpoint_move_file))
        .route("/tree/", get(metadata::endpoint_get_file_tree))
        .routes(routes!(metadata::endpoint_get_file_tree))
        .routes(routes!(metadata::endpoint_get_single_file_metadata))
        .with_state(database)
}

#[derive(Deserialize, ToSchema)]
struct RequestTimestamp {
    timestamp: Option<String>,
    seconds: Option<i64>,
}

impl TryFrom<RequestTimestamp> for DateTime<Utc> {
    type Error = ApiError;

    fn try_from(value: RequestTimestamp) -> std::result::Result<Self, Self::Error> {
        Ok(match (value.timestamp, value.seconds) {
            (Some(timestamp), _) => DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc),
            (None, Some(seconds)) => Utc::now() - TimeDelta::seconds(seconds),
            _ => Utc::now(),
        })
    }
}
