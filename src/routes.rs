use aide::{axum::{routing::get_with, ApiRouter, IntoApiResponse}, transform::TransformOperation};
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;

use crate::database::AppState;



pub fn file_routes(state: AppState) -> ApiRouter {
    ApiRouter::new()
        .api_route("/", get_with(get_files, get_files_docs))
        .with_state(state)
}

#[derive(Deserialize)]
struct NewTodo {
    /// The description for the new Todo.
    description: String,
}

async fn get_files(State(app): State<AppState>) -> impl IntoApiResponse {
    StatusCode::OK
}

fn get_files_docs(op: TransformOperation) -> TransformOperation {
    op.description("List all Todo items.")
}

async fn upload_file(
    State(app): State<AppState>,
    Json(todo): Json<NewTodo>,
) -> impl IntoApiResponse {
    // let id = Uuid::new_v4();
    // app.todos.lock().unwrap().insert(
    //     id,
    //     TodoItem {
    //         complete: false,
    //         description: todo.description,
    //         id,
    //     },
    // );

    // (StatusCode::CREATED, Json(TodoCreated { id }))
}

