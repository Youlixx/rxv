use axum::{extract::{Path as ExtractPath, State}, routing::get};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{database::AppState, response::{ApiResponse, ApiResult}};



pub fn router(storage: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/tree/", get(tree))
        .route("/tree/{*path}", get(tree))
        // .routes(routes!(tree))
        .with_state(storage)
}


#[utoipa::path(
    get,
    path = "/tree/{*path}",
    tag = "fs",
    responses(
        (status = 200, description = "The filepaths were successfully returned")
    )
)]
async fn tree(
    State(storage): State<AppState>,
    path: Option<ExtractPath<String>>,
) -> ApiResult<()> {
    dbg!(path);
    Ok(ApiResponse::success(()))
}