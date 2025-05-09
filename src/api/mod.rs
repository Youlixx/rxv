use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;

use crate::database::FileDatabase;

pub mod files;
pub mod response;

#[derive(OpenApi)]
#[openapi(
    tags(
        (name = "files", description = "Files operations")
    )
)]
struct ApiDoc;

pub fn get_router(database: FileDatabase) -> Router {
    // TODO: maybe use a Arc here? check if it is ok to clone database.database?
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(files::router(database.clone()))
        .split_for_parts();

    let router =
        router.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api.clone()));

    router
}
