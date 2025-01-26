use std::path::{Path, PathBuf};

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::fs;

use crate::response::Result;

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

    pub async fn add_new_file_to_storage(
        &self,
        path_storage: impl AsRef<Path>,
        path_temp_file: impl AsRef<Path>,
        hash_md5: &str,
        hash_sha256: &str,
    ) -> Result<()> {
        // TODO check if the file exists already in the store first
        let path_copy = self.path_files.join(hash_sha256);

        // TODO: we must check the validity of the path, because it may
        // contains stuff like .., probably should canonicalize.
        let path_storage = path_storage.as_ref().to_string_lossy().to_string();
        let current_time = Utc::now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        sqlx::query!(
            "
            UPDATE paths
            SET valid_until = ?
            WHERE path = ? AND valid_until IS NULL;
            ",
            current_time,
            path_storage
        )
        .execute(&mut *transaction)
        .await?;

        if !fs::try_exists(&path_copy).await? {
            fs::copy(path_temp_file, path_copy).await?;

            sqlx::query!(
                "
                INSERT INTO files (original_file_name, size, md5_hash, sha256_hash, upload_date)
                VALUES (?, ?, ?, ?, ?)
                ",
                "placeholder",
                1000,
                hash_md5,
                hash_sha256,
                current_time
            )
            .execute(&mut *transaction)
            .await?;
        };

        let file_id = sqlx::query!(
            "
            SELECT id FROM files WHERE sha256_hash = ?;
            ",
            hash_sha256
        )
        .fetch_one(&mut *transaction)
        .await?;

        sqlx::query!(
            "
            INSERT INTO paths (file_id, path, valid_since, valid_until)
            VALUES (?, ?, ?, NULL);
            ",
            file_id.id,
            path_storage,
            current_time
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
    }
}
