use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tokio::fs;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct AppState {
    database: SqlitePool,
    path_files: PathBuf,
}

impl AppState {
    const DATABASE_FILE_NAME: &str = "rxv.db";
    const STORAGE_FOLDER_NAME: &str = "files";

    /// Create and initialize the SQLite database with default tables.
    ///
    /// The given database url must point to either memory or an existing file.
    /// If the file is not existing, the connection will fail.
    pub async fn new(path_root: impl AsRef<Path>) -> Result<Self> {
        // TODO check that the path is absolute.
        let path_root = path_root.as_ref();
        if !path_root.exists() {
            fs::create_dir_all(path_root).await?;
        }

        let path_database = path_root.join(AppState::DATABASE_FILE_NAME);
        if !path_database.exists() {
            fs::File::create(&path_database).await?;
        }

        let path_files = path_root.join(AppState::STORAGE_FOLDER_NAME);
        if !path_files.exists() {
            fs::create_dir(&path_files).await?;
        }

        let database_url = String::from("sqlite:") + &path_database.to_string_lossy();
        let database = SqlitePool::connect(&database_url).await?;
        let mut transaction = database.begin().await?;

        sqlx::query!(
            "
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                original_file_name TEXT NOT NULL,
                size INTEGER UNIQUE NOT NULL,
                md5_hash TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                upload_date TEXT NOT NULL
            )
            ",
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query!(
            "
            CREATE TABLE IF NOT EXISTS paths (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                valid_since TEXT NOT NULL,
                valid_until TEXT,
                FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
            )
            ",
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(Self {
            database,
            path_files,
        })
    }
}
