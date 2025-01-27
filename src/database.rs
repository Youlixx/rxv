use std::{
    collections::HashMap, fs::File, path::{Path, PathBuf}
};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use flate2::{write::GzEncoder, Compression};
use serde::Serialize;
use sqlx::SqlitePool;
use tar::Builder;
use tokio::fs;
use utoipa::ToSchema;

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

// TODO might be good to also give file info, like HashMap<String, FileInfo>
#[derive(Serialize, ToSchema)]
pub struct FileSystemState {
    paths: HashMap<String, i64>,
}

impl AppState {
    pub async fn get_filesystem_at<T>(&self, timestamp: DateTime<T>) -> Result<FileSystemState>
    where
        T: TimeZone,
    {
        let timestamp = timestamp.to_rfc3339();

        let paths = sqlx::query!(
            "
            SELECT path, file_id FROM paths
            WHERE ? >= valid_since AND ? < COALESCE(valid_until, '9999-12-31T23:59:59Z');
            ",
            timestamp,
            timestamp
        )
        .fetch_all(&self.database)
        .await?
        .into_iter()
        .map(|record| (record.path, record.file_id))
        .collect();

        Ok(FileSystemState { paths })
    }
}

pub enum TimePoint {
    Absolute(DateTime<Utc>),
    Relative(TimeDelta),
}

impl TimePoint {
    fn to_absolute(self) -> DateTime<Utc> {
        match self {
            TimePoint::Absolute(timestamp) => timestamp,
            TimePoint::Relative(delta) => Utc::now() - delta,
        }
    }
}

impl AppState {
    pub async fn download_file_from_storage(
        &self,
        path_storage: impl AsRef<Path>,
        time_point: TimePoint,
    ) -> Result<PathBuf> {
        let timestamp = time_point.to_absolute().to_rfc3339();
        let path_storage = path_storage.as_ref().to_path_buf();
        let path_string = path_storage.to_string_lossy();

        let file_hash = sqlx::query!(
            "
            SELECT files.sha256_hash FROM files
            INNER JOIN paths ON files.id == paths.file_id
            WHERE ? >= paths.valid_since AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z') AND paths.path == ?;
            ",
            timestamp,
            timestamp,
            path_string
        )
        .fetch_optional(&self.database)
        .await?
        .ok_or(Error::FileNotFound(path_storage))?;

        Ok(self.path_files.join(file_hash.sha256_hash))
    }

    pub async fn download_folder_from_storage<P>(
        &self,
        path_storage: P,
        time_point: TimePoint,
    ) -> Result<PathBuf>
    where
        P: AsRef<Path>,
    {
        let timestamp = time_point.to_absolute().to_rfc3339();
        let path_storage = path_storage.as_ref().to_path_buf();
        let path_string = format!("{}%", path_storage.to_string_lossy());

        // TODO this is terrible :)
        let path_output_file = PathBuf::from("/tmp/archive.tar.gz");
        let output_file = File::create(&path_output_file)?;
        let encoder = GzEncoder::new(output_file, Compression::default());
        let mut builder = Builder::new(encoder);

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
            let path_storage = self.path_files.join(record.sha256_hash);
            builder.append_path_with_name(path_storage, record.path)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

        // Should probably return this as a TempFile and implement Deref<W> for
        // TempFile or something like this. TempFile should probably be generic
        // over tokio File and std File.
        builder.into_inner()?.finish()?;

        Ok(path_output_file)
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
    pub async fn add_new_file_to_storage(
        &self,
        path_storage: impl AsRef<Path>,
        path_temp_file: impl AsRef<Path>,
        file_info: FileInfo,
    ) -> Result<()> {
        let path_copy = self.path_files.join(&file_info.hash_sha256);

        // TODO: we must check the validity of the path, because it may
        // contains stuff like .., probably should canonicalize.
        let path_storage = path_storage.as_ref().to_string_lossy().to_string();
        let current_time = Utc::now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        // TODO: if the given path points to the exact same file, then we
        // should not update the path table, or it will lead to some stuff like
        // file_id: 1, valid_since: 10, valid_until: 40
        // file_id: 1, valid_since: 40, valid_until: None
        // which should be merged into
        // file_id: 1, valid_since: 10, valid_until: None
        // i.e. do nothing on the paths table.
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
                file_info.original_name,
                file_info.size_in_bytes,
                file_info.hash_md5,
                file_info.hash_sha256,
                current_time
            )
            .execute(&mut *transaction)
            .await?;
        };

        let file_id = sqlx::query!(
            "
            SELECT id FROM files WHERE sha256_hash = ?;
            ",
            file_info.hash_sha256
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
