use std::path::PathBuf;

// use aide::{
//     axum::{routing::get_with, ApiRouter, IntoApiResponse},
//     transform::TransformOperation,
// };
use axum::{
    extract::{DefaultBodyLimit, Multipart, State}, http::StatusCode, response::IntoResponse, routing::get, Json, Router
};
use serde::{Deserialize, Serialize};
use utoipa::{openapi::schema, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::database::AppState;

pub fn file_routes(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(get_files, upload_file))
        // TODO configurable max size
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024 * 1024))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "",
    tag = "yo",
    responses(
        (status = 200, description = "List all todos successfully")
    )
)]
async fn get_files(State(app): State<AppState>) -> impl IntoResponse {
    StatusCode::OK
}

// fn get_files_docs(op: TransformOperation) -> TransformOperation {
//     op.description("List all Todo items.")
// }

#[derive(Deserialize)]
struct FileUploadRequest {
    path: PathBuf,
}

#[derive(Deserialize, ToSchema)]
#[allow(unused)]
struct HelloForm {
    name: String,
    #[schema(format = Binary, content_media_type = "application/octet-stream")]
    file: String,
}
// #[derive(MultipartForm, ToSchema, Debug)]
// pub struct Upload {
//     #[schema(value_type = String, format = Binary)]
//     #[multipart(limit = "512 MiB")]
//     pub file_content: MultipartBytes,
//     pub metadata: Option<Json<Metadata>>,
// }

#[utoipa::path(
    post,
    request_body(content = inline(HelloForm), content_type = "multipart/form-data"),
    path = "",
    tag = "yo",
    responses(
        (status = 200, description = "List all todos successfully")
    )
)]
async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut name: Option<String> = None;

    let mut content_type: Option<String> = None;
    let mut size: usize = 0;
    let mut file_name: Option<String> = None;

    while let Some(mut field) = multipart.next_field().await.unwrap() {
        // let name = field.name().unwrap().to_string();
        // let data = field.bytes().await.unwrap();

        // println!("Length of `{}` is {} bytes", name, data.len());

        let field_name = field.name();

        match &field_name {
            Some("name") => {
                name = Some(field.text().await.expect("should be text for name field"));
            }
            Some("file") => {
                file_name = field.file_name().map(ToString::to_string);
                content_type = field.content_type().map(ToString::to_string);
                let bytes = field.bytes().await.expect("should be bytes for file field");
                size = bytes.len();
            }
            _ => (),
        };

    }

    let x = format!(
        "name: {}, content_type: {}, size: {}, file_name: {}",
        name.unwrap_or_default(),
        content_type.unwrap_or_default(),
        size,
        file_name.unwrap_or_default()
    );

    println!("{}", x);

    StatusCode::CREATED
}

// fn upload_file_docs(op: TransformOperation) -> TransformOperation {
//     op.description("Upload a new file.")
// }
