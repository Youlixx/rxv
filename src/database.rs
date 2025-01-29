use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
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

#[derive(Debug)]
pub enum TargetDownload {
    FullStorage,
    File(PathBuf),
    Folder(PathBuf),
}

impl<P> From<Option<P>> for TargetDownload
where
    P: AsRef<Path>,
{
    fn from(value: Option<P>) -> Self {
        match value {
            Some(path) => {
                let path = path.as_ref().to_string_lossy().to_string();

                if path.ends_with("/") {
                    TargetDownload::Folder(PathBuf::from(path))
                } else {
                    TargetDownload::File(PathBuf::from(path))
                }
            }
            None => TargetDownload::FullStorage,
        }
    }
}

#[derive(Debug)]
pub enum FileList {
    None,
    SingleFile(PathBuf),
    MultipleFile(Vec<(PathBuf, PathBuf)>),
}

impl From<Vec<(PathBuf, PathBuf)>> for FileList {
    fn from(value: Vec<(PathBuf, PathBuf)>) -> Self {
        if value.is_empty() {
            FileList::None
        } else {
            FileList::MultipleFile(value)
        }
    }
}

impl From<Option<PathBuf>> for FileList {
    fn from(value: Option<PathBuf>) -> Self {
        match value {
            Some(path) => FileList::SingleFile(path),
            None => FileList::None,
        }
    }
}

impl AppState {
    pub async fn get_file_paths(
        &self,
        target_download: TargetDownload,
        timestamp: DateTime<Utc>,
    ) -> Result<FileList> {
        dbg!(&target_download);
        let timestamp = timestamp.to_rfc3339();

        let files: FileList = match target_download {
            TargetDownload::FullStorage => sqlx::query!(
                "
                SELECT files.sha256_hash, paths.path FROM files
                INNER JOIN paths ON files.id == paths.file_id
                WHERE ? >= paths.valid_since
                    AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z');
                ",
                timestamp,
                timestamp
            )
            .fetch_all(&self.database)
            .await?
            .into_iter()
            .map(|record| {
                (
                    self.path_files.join(record.sha256_hash),
                    PathBuf::from(record.path),
                )
            })
            .collect::<Vec<_>>()
            .into(),
            TargetDownload::File(path) => {
                let path_string = path.to_string_lossy();
                sqlx::query!(
                    "
                    SELECT files.sha256_hash FROM files
                    INNER JOIN paths ON files.id == paths.file_id
                    WHERE ? >= paths.valid_since
                        AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                        AND paths.path == ?;
                    ",
                    timestamp,
                    timestamp,
                    path_string
                )
                .fetch_optional(&self.database)
                .await?
                .map(|file_hash| self.path_files.join(file_hash.sha256_hash))
                .into()
            }
            TargetDownload::Folder(path) => {
                let path_string = format!("{}%", path.to_string_lossy());

                sqlx::query!(
                    "
                    SELECT files.sha256_hash, paths.path FROM files
                    INNER JOIN paths ON files.id == paths.file_id
                    WHERE ? >= paths.valid_since
                        AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                        AND paths.path LIKE ?;
                    ",
                    timestamp,
                    timestamp,
                    path_string
                )
                .fetch_all(&self.database)
                .await?
                .into_iter()
                .map(|record| {
                    (
                        self.path_files.join(record.sha256_hash),
                        PathBuf::from(record.path),
                    )
                })
                .collect::<Vec<_>>()
                .into()
            }
        };

        Ok(files)
    }
}

#[derive(Debug)]
pub struct FileInfo {
    pub original_name: String,
    pub size_in_bytes: i64,
    pub hash_md5: String,
    pub hash_sha256: String,
}

impl AppState {
    /// Asynchronously adds a new file to storage.
    ///
    /// This function takes a path to physical file, the path of the new file
    /// relative to the storage and information about the file, and adds the
    /// file to the storage. Once this function completes, the original file
    /// may be deleted.
    pub async fn add_new_file_to_storage(
        &self,
        path_file: impl AsRef<Path>,
        path_storage: impl AsRef<Path>,
        file_info: FileInfo,
    ) -> Result<()> {
        let path_copy = self.path_files.join(&file_info.hash_sha256);
        let path_storage = path_storage.as_ref().to_string_lossy().to_string();
        let timestamp = Utc::now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        let file_id = if !fs::try_exists(&path_copy).await? {
            fs::copy(path_file, path_copy).await?;

            Some(
                sqlx::query!(
                    "
                INSERT INTO files (original_file_name, size, md5_hash, sha256_hash, upload_date)
                VALUES (?, ?, ?, ?, ?)
                ",
                    file_info.original_name,
                    file_info.size_in_bytes,
                    file_info.hash_md5,
                    file_info.hash_sha256,
                    timestamp
                )
                .execute(&mut *transaction)
                .await?
                .last_insert_rowid() as i64,
            )
        } else {
            // If a user attempts to create a new path that already exists and
            // points to the same file, the transaction should be canceled.
            // This approach prevents the same path from being invalidated and
            // then revalidated consecutively at the same timestamp.
            let same_file_with_same_path = sqlx::query!(
                "
                SELECT COUNT(paths.id) as count FROM files
                INNER JOIN paths ON files.id == paths.file_id
                WHERE paths.valid_until IS NULL
                    AND paths.path == ?
                    AND files.sha256_hash = ?;
                ",
                path_storage,
                file_info.hash_sha256
            )
            .fetch_one(&mut *transaction)
            .await?
            .count;

            if same_file_with_same_path != 0 {
                transaction.rollback().await?;
                return Ok(());
            }

            sqlx::query!(
                "SELECT id FROM files WHERE sha256_hash = ?;",
                file_info.hash_sha256
            )
            .fetch_one(&mut *transaction)
            .await?
            .id
        };

        sqlx::query!(
            "
            UPDATE paths
            SET valid_until = ?
            WHERE path = ? AND valid_until IS NULL;
            ",
            timestamp,
            path_storage
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query!(
            "
            INSERT INTO paths (file_id, path, valid_since, valid_until)
            VALUES (?, ?, ?, NULL);
            ",
            file_id,
            path_storage,
            timestamp
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
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
