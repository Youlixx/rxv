pub mod download;
pub mod upload;

use std::path::{Path, PathBuf};

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::fs;

use crate::response::{Error, Result};

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
                size INTEGER NOT NULL,
                md5_hash TEXT NOT NULL,
                sha256_hash TEXT UNIQUE NOT NULL,
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

impl AppState {
    pub async fn delete_file_from_storage(&self, path_storage: impl AsRef<Path>) -> Result<()> {
        let path_storage = path_storage.as_ref().to_path_buf();
        let path_string = path_storage.to_string_lossy();
        let timestamp = Utc::now().to_rfc3339();

        let files_deleted = if !path_string.ends_with("/") {
            sqlx::query!(
                "
                UPDATE paths
                SET valid_until = ?
                WHERE path = ? AND valid_until IS NULL;
                ",
                timestamp,
                path_string
            )
            .execute(&self.database)
            .await?
            .rows_affected()
        } else {
            let path_string = format!("{}%", path_string);

            sqlx::query!(
                "
                UPDATE paths
                SET valid_until = ?
                WHERE path LIKE ? AND valid_until IS NULL;
                ",
                timestamp,
                path_string
            )
            .execute(&self.database)
            .await?
            .rows_affected()
        };

        if files_deleted == 0 {
            Err(Error::FileNotFound(path_storage))
        } else {
            Ok(())
        }
    }
}
