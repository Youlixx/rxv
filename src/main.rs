pub mod database;
pub mod error;
mod routes;
// mod docs;

use std::sync::Arc;

// use aide::{axum::ApiRouter, openapi::OpenApi, transform::TransformOpenApi};
use axum::{Extension, Router};
use database::AppState;
use routes::file_routes;
use tokio::net::TcpListener;
use utoipa::{openapi, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;
use std::net::{Ipv4Addr, SocketAddr};

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


    // Router::new()
    //     .

    // let app = ApiRouter::new()
    //     .nest_api_service("/files", file_routes(state.clone()))
    //     .nest_api_service("/docs", docs_routes(state.clone()))
    //     .finish_api_with(&mut api, api_docs)
    //     .layer(Extension(Arc::new(api))) // Arc is very important here or you will face massive memory and performance issues
    //     .with_state(state);

    // let app = Router::new()
    //     .nest("/files", file_routes(state.clone()));

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api/v1/files", file_routes(state.clone()))
        .split_for_parts();

    let router = router
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api.clone()))
        // .merge(Redoc::with_url("/redoc", api.clone()))
        // // There is no need to create `RapiDoc::with_openapi` because the OpenApi is served
        // // via SwaggerUi instead we only make rapidoc to point to the existing doc.
        // .merge(RapiDoc::new("/api-docs/openapi.json").path("/rapidoc"))
        // // Alternative to above
        // // .merge(RapiDoc::with_openapi("/api-docs/openapi2.json", api).path("/rapidoc"))
        // .merge(Scalar::with_url("/scalar", api))
        ;

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8000));
    let listener = TcpListener::bind(&address).await.unwrap();
    println!("Example docs are accessible at http://127.0.0.1:8000/docs");
    axum::serve(listener, router.into_make_service()).await.unwrap()




    // let listener = TcpListener::bind("0.0.0.0:8000").await.unwrap();

    // axum::serve(listener, app).await.unwrap();
}

// fn api_docs(api: TransformOpenApi) -> TransformOpenApi {
//     api.title("Aide axum Open API")
//         .summary("An example Todo application")
//     // .description(include_str!("README.md"))
//     // .tag(Tag {
//     //     name: "todo".into(),
//     //     description: Some("Todo Management".into()),
//     //     ..Default::default()
//     // })
//     // .security_scheme(
//     //     "ApiKey",
//     //     aide::openapi::SecurityScheme::ApiKey {
//     //         location: aide::openapi::ApiKeyLocation::Header,
//     //         name: "X-Auth-Key".into(),
//     //         description: Some("A key that is ignored.".into()),
//     //         extensions: Default::default(),
//     //     },
//     // )
//     // .default_response_with::<Json<AppError>, _>(|res| {
//     //     res.example(AppError {
//     //         error: "some error happened".to_string(),
//     //         error_details: None,
//     //         error_id: Uuid::nil(),
//     //         // This is not visible.
//     //         status: StatusCode::IM_A_TEAPOT,
//     //     })
//     // })
// }
