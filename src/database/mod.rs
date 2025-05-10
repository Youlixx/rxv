use std::{
    io,
    path::{Path, PathBuf},
};

use error::Result;
use sqlx::SqlitePool;
use tokio::fs;

pub mod error;
pub mod virtual_path;

pub mod delete_file;
pub mod get_file;
pub mod get_metadata;
pub mod save_file;

#[derive(Debug, Clone)]
pub struct FileDatabase {
    database: SqlitePool,
    path_files: PathBuf,
}

impl FileDatabase {
    const DATABASE_FILE_NAME: &str = "rxv.db";
    const STORAGE_FOLDER_NAME: &str = "files";

    /// Set up the database tables.
    ///
    /// This function creates the necessary tables for storing file information and
    /// paths. There are two tables:
    /// - `files`: Stores information about the files, including their original name,
    ///   size, hash, and upload date.
    /// - `paths`: Stores the paths associated with each file, including the valid time
    ///   range for each path.
    async fn setup_tables(&self) -> Result<()> {
        let mut transaction = self.database.begin().await?;

        sqlx::query!(
            "
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                original_file_name TEXT NOT NULL,
                size INTEGER NOT NULL,
                hash TEXT NOT NULL,
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

        Ok(())
    }

    /// Open the database at the given path.
    ///
    /// The given path must be absolute. If the database does not exist yet, it will be
    /// created with default tables.
    pub async fn open(path_root: impl AsRef<Path>) -> Result<Self> {
        if !path_root.as_ref().is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The database path must be absolute",
            )
            .into());
        }

        let path_root = path_root.as_ref();
        if !path_root.exists() {
            fs::create_dir_all(path_root).await?;
        }

        let path_database = path_root.join(FileDatabase::DATABASE_FILE_NAME);
        if !path_database.exists() {
            fs::File::create(&path_database).await?;
        }

        let path_files = path_root.join(FileDatabase::STORAGE_FOLDER_NAME);
        if !path_files.exists() {
            fs::create_dir(&path_files).await?;
        }

        let database_url = String::from("sqlite:") + &path_database.to_string_lossy();

        let database = FileDatabase {
            database: SqlitePool::connect(&database_url).await?,
            path_files: path_files.to_path_buf(),
        };

        database.setup_tables().await?;

        Ok(database)
    }

    fn get_physical_file_path(&self, hash: &str) -> PathBuf {
        self.path_files.join(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::{FileDatabase, error::Result, save_file::FileMetadata, virtual_path::VirtualPath};
    use async_tempfile::{TempDir, TempFile};
    use chrono::{DateTime, Utc};
    use md5::Digest;
    use sha2::Sha256;
    use std::{fs::File, io::Read};
    use tokio::io::AsyncWriteExt;

    #[derive(Debug, Eq, PartialEq)]
    pub struct FileDatabaseFile {
        pub original_file_name: String,
        pub upload_date: DateTime<Utc>,
        pub content: Box<[u8]>,
    }

    #[derive(Debug, Eq, PartialEq)]
    pub struct FileDatabasePath {
        pub file_id: usize,
        pub path: VirtualPath,
        pub valid_since: DateTime<Utc>,
        pub valid_until: Option<DateTime<Utc>>,
    }

    #[derive(Debug, Eq, PartialEq)]
    pub struct FileDatabaseState {
        pub files: Vec<FileDatabaseFile>,
        pub paths: Vec<FileDatabasePath>,
    }

    #[derive(Debug)]
    pub enum FileOperation {
        Save {
            original_file_name: String,
            virtual_path: VirtualPath,
            content: Box<[u8]>,
            timestamp: DateTime<Utc>,
        },
        Delete {
            virtual_path: VirtualPath,
            timestamp: DateTime<Utc>,
        },
    }

    pub fn get_timestamp(seconds: usize) -> DateTime<Utc> {
        return DateTime::from_timestamp(seconds as i64, 0)
            .unwrap()
            .to_utc();
    }

    pub fn get_hash(file_content: &[u8]) -> String {
        let mut hash = Sha256::new();
        hash.update(file_content);
        hex::encode(hash.finalize())
    }

    pub async fn get_database_state(database: &FileDatabase) -> Result<FileDatabaseState> {
        let files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?
            .into_iter()
            .map(|file| -> Result<FileDatabaseFile> {
                let path_file = database.path_files.join(&file.hash);
                let mut content = Vec::new();
                File::open(path_file)?.read_to_end(&mut content)?;

                assert_eq!(file.hash, get_hash(&content));
                assert_eq!(file.size as usize, content.len());

                Ok(FileDatabaseFile {
                    original_file_name: file.original_file_name,
                    upload_date: DateTime::parse_from_rfc3339(&file.upload_date)?.to_utc(),
                    content: content.into_boxed_slice(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?
            .into_iter()
            .map(|path| -> Result<FileDatabasePath> {
                Ok(FileDatabasePath {
                    file_id: path.file_id as usize,
                    path: path.path.into(),
                    valid_since: DateTime::parse_from_rfc3339(&path.valid_since)?.to_utc(),
                    valid_until: match path.valid_until {
                        Some(timestamp) => Some(DateTime::parse_from_rfc3339(&timestamp)?.to_utc()),
                        None => None,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(FileDatabaseState { files, paths })
    }

    pub async fn setup_test_database(
        operations: Vec<FileOperation>,
    ) -> Result<(TempDir, FileDatabase)> {
        let test_dir = TempDir::new().await.unwrap();
        let database = FileDatabase::open(test_dir.dir_path()).await?;

        for operation in operations {
            match operation {
                FileOperation::Save {
                    original_file_name,
                    virtual_path,
                    content,
                    timestamp,
                } => {
                    let mut file = TempFile::new().await.unwrap();
                    file.write(&content).await?;

                    database
                        .save_file(
                            file.file_path(),
                            virtual_path,
                            timestamp,
                            FileMetadata {
                                original_file_name: original_file_name,
                                size_in_bytes: content.len(),
                                hash: get_hash(&content),
                            },
                        )
                        .await?;
                }
                FileOperation::Delete {
                    virtual_path,
                    timestamp,
                } => {
                    database.delete_file(virtual_path, timestamp).await?;
                }
            }
        }

        Ok((test_dir, database))
    }

    pub async fn check_database_state(
        operations: Vec<FileOperation>,
        expected_state: FileDatabaseState,
    ) -> Result<()> {
        let (_test_dir, database) = setup_test_database(operations).await?;
        assert_eq!(get_database_state(&database).await?, expected_state);
        Ok(())
    }
}
