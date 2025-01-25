use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use sqlx::SqlitePool;

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
            fs::create_dir_all(path_root)?;
        }

        let path_database = path_root.join(AppState::DATABASE_FILE_NAME);
        if !path_database.exists() {
            File::create(&path_database)?;
        }

        let path_files = path_root.join(AppState::STORAGE_FOLDER_NAME);
        if !path_files.exists() {
            fs::create_dir(&path_files)?;
        }

        let database_url = String::from("sqlite:") + &path_database.to_string_lossy();
        let database = SqlitePool::connect(&database_url).await?;

        sqlx::query(
            "
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL,
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
            ",
        )
        .execute(&database)
        .await?;

        Ok(Self {
            database,
            path_files,
        })
    }

    // pub async fn upload_file()
}
