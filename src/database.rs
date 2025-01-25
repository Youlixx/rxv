use sqlx::SqlitePool;

use crate::error::Result;

#[derive(Debug)]
pub struct AppState {
    database: SqlitePool,
}

impl AppState {
    /// Create and initialize the SQLite database with default tables.
    ///
    /// The given database url must point to either memory or an existing file.
    /// If the file is not existing, the connection will fail.
    pub async fn new(database_url: &str) -> Result<Self> {
        let database = SqlitePool::connect(database_url).await?;

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
        .await?;

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
        .await?;

        Ok(Self { database })
    }
}
