mod database;
mod response;
mod routes;

use routes::{files, fs};

use database::AppState;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::TcpListener;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    // modifiers(&SecurityAddon),
    tags(
        (name = "yo", description = "Todo items management API")
    )
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    let state = AppState::new("/storage").await.unwrap();

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(files::router(state.clone()))
        .nest("/fs", fs::router(state.clone()))
        .split_for_parts();

    let router =
        router.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api.clone()));

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8000));
    let listener = TcpListener::bind(&address).await.unwrap();

    axum::serve(listener, router.into_make_service())
        .await
        .unwrap()
}
