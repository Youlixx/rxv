use api::get_router;
use database::FileDatabase;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::TcpListener;

mod api;
mod database;

#[tokio::main]
async fn main() {
    let database = FileDatabase::open("/storage").await.unwrap();
    let router = get_router(database);

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8000));
    let listener = TcpListener::bind(&address).await.unwrap();

    axum::serve(listener, router.into_make_service())
        .await
        .unwrap()
}
