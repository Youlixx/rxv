pub mod database;
pub mod error;

use database::AppState;


#[tokio::main]
async fn main() {
    let db = AppState::new("sqlite:/db.sqlite").await.unwrap();
}
