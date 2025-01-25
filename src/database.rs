use sqlx::SqlitePool;

#[derive(Debug)]
pub struct AppState {
    database: SqlitePool,
}

impl AppState {
    pub async fn new(database_url: &str) -> Option<Self> {
        let database = SqlitePool::connect(database_url).await.ok()?;

        sqlx::query!(
            "
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                original_file_name TEXT NOT NULL,
                size INTEGER UNIQUE NOT NULL,
                md5_hash TEXT NOT NULL,
                sha256_hash TEXT NOT NULL
            )
            ",
        )
        .execute(&database)
        .await
        .ok()?;

        sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS paths (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    file_id INTEGER NOT NULL,
                    path TEXT NOT NULL,
                    date TEXT NOT NULL,
                    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
                )
                "
        ).execute(&database)
        .await
        .ok()?;

        Some(Self { database })
    }
}
