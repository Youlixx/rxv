pub mod database;
pub mod error;
mod routes;
mod docs;

use std::sync::Arc;

use aide::{axum::ApiRouter, openapi::OpenApi, transform::TransformOpenApi};
use axum::Extension;
use database::AppState;
use docs::docs_routes;
use routes::file_routes;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    aide::generate::extract_schemas(true);

    let state = AppState::new("sqlite:/db.sqlite").await.unwrap();

    let mut api = OpenApi::default();

    let app = ApiRouter::new()
        .nest_api_service("/files", file_routes(state.clone()))
        .nest_api_service("/docs", docs_routes(state.clone()))
        .finish_api_with(&mut api, api_docs)
        .layer(Extension(Arc::new(api))) // Arc is very important here or you will face massive memory and performance issues
        .with_state(state);

    println!("Example docs are accessible at http://127.0.0.1:8000/docs");

    let listener = TcpListener::bind("0.0.0.0:8000").await.unwrap();

    axum::serve(listener, app).await.unwrap();
}

fn api_docs(api: TransformOpenApi) -> TransformOpenApi {
    api.title("Aide axum Open API")
        .summary("An example Todo application")
    // .description(include_str!("README.md"))
    // .tag(Tag {
    //     name: "todo".into(),
    //     description: Some("Todo Management".into()),
    //     ..Default::default()
    // })
    // .security_scheme(
    //     "ApiKey",
    //     aide::openapi::SecurityScheme::ApiKey {
    //         location: aide::openapi::ApiKeyLocation::Header,
    //         name: "X-Auth-Key".into(),
    //         description: Some("A key that is ignored.".into()),
    //         extensions: Default::default(),
    //     },
    // )
    // .default_response_with::<Json<AppError>, _>(|res| {
    //     res.example(AppError {
    //         error: "some error happened".to_string(),
    //         error_details: None,
    //         error_id: Uuid::nil(),
    //         // This is not visible.
    //         status: StatusCode::IM_A_TEAPOT,
    //     })
    // })
}
