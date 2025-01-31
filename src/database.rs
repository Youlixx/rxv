use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::fs;

use crate::{
    path::StoragePath,
    response::{Error, Result},
};

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
pub enum FileList {
    None,
    SingleFile(PathBuf),
    MultipleFile(Vec<(PathBuf, StoragePath)>),
}

impl From<Vec<(PathBuf, StoragePath)>> for FileList {
    fn from(value: Vec<(PathBuf, StoragePath)>) -> Self {
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
    /// Retrieve the list of live file that match the given root path.
    ///
    /// This function can be used to retrieve the actual physical path to the
    /// files on the disk. Three cases are handled by this function :
    /// - the target storage path is the actual root path, in which case all
    ///   the live storage files are returned.
    /// - the target storage path points to a single file, in which case the
    ///   actual path to the file is returned if it exists.
    /// - the target storage path points to a folder (must end with '/'), in
    ///   which case all that path to all the live files are returned.
    /// Note that when dealing with the root path or folders, this function
    /// recursively retrieve all the file, including sub-folders.
    pub async fn get_file_paths(
        &self,
        base_path: &StoragePath,
        timestamp: DateTime<Utc>,
    ) -> Result<FileList> {
        let timestamp = timestamp.to_rfc3339();

        let files = if base_path.is_dir() {
            let path_wildcard = base_path.get_sql_matching_pattern();
            let query = sqlx::query!(
                "
                SELECT files.sha256_hash, paths.path FROM files
                INNER JOIN paths ON files.id == paths.file_id
                WHERE ? >= paths.valid_since
                    AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                    AND paths.path LIKE ?;
                ",
                timestamp,
                timestamp,
                path_wildcard
            );

            query
                .fetch_all(&self.database)
                .await?
                .into_iter()
                .map(|record| {
                    (
                        self.path_files.join(record.sha256_hash),
                        StoragePath::from(record.path),
                    )
                })
                .collect::<Vec<_>>()
                .into()
        } else {
            let storage_relative_path_file = base_path.to_str();

            let query = sqlx::query!(
                "
                SELECT files.sha256_hash FROM files
                INNER JOIN paths ON files.id == paths.file_id
                WHERE ? >= paths.valid_since
                    AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                    AND paths.path == ?;
                ",
                timestamp,
                timestamp,
                storage_relative_path_file
            );

            query
                .fetch_optional(&self.database)
                .await?
                .map(|file_hash| self.path_files.join(file_hash.sha256_hash))
                .into()
        };

        Ok(files)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
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

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        iter::zip,
        path::Path,
        time::Duration,
    };

    use async_tempfile::{TempDir, TempFile};
    use chrono::{DateTime, Utc};
    use md5::{Digest, Md5};
    use sha2::Sha256;
    use tokio::{
        fs::{self, File},
        io::{AsyncReadExt, AsyncWriteExt},
        time::sleep,
    };

    use crate::{
        database::{AppState, FileInfo},
        response::Result,
    };

    /// Create a temporary dummy file.
    async fn create_dummy_file(file_content: &[u8]) -> Result<(TempFile, FileInfo)> {
        let mut file = TempFile::new().await?;
        file.write(file_content).await?;

        let mut hash_md5 = Md5::new();
        hash_md5.update(file_content);

        let mut hash_sha256 = Sha256::new();
        hash_sha256.update(file_content);

        Ok((
            file,
            FileInfo {
                original_name: "my_file.txt".into(),
                size_in_bytes: file_content.len() as i64,
                hash_md5: hex::encode(hash_md5.finalize()),
                hash_sha256: hex::encode(hash_sha256.finalize()),
            },
        ))
    }

    /// Check that the content of the given file match an expected content.
    async fn check_file_content(path_file: impl AsRef<Path>, file_content: &[u8]) -> Result<()> {
        let path_file = path_file.as_ref().to_path_buf();

        assert!(fs::try_exists(&path_file).await?);
        assert!(path_file.is_file());

        let mut saved_content = Vec::new();
        File::open(path_file)
            .await?
            .read_to_end(&mut saved_content)
            .await?;

        assert_eq!(saved_content.len(), file_content.len());
        assert_eq!(saved_content, file_content);

        Ok(())
    }

    /// Test checking that saving a single file to the storage works as
    /// intended.
    ///
    /// The intended behavior is that both the paths and files table contain
    /// exactly a single entry for the new file. The file should also be
    /// present in the storage folder.
    #[tokio::test]
    async fn test_save_single_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";

        let (file, file_info) = create_dummy_file(file_content).await?;

        database
            .add_new_file_to_storage(
                file.file_path(),
                "my_files/helloworld.txt",
                file_info.clone(),
            )
            .await?;

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), 1);

        let inserted_file = inserted_files
            .first()
            .expect("The table must contains exactly one file.");

        assert_eq!(inserted_file.original_file_name, file_info.original_name);
        assert_eq!(inserted_file.size, file_info.size_in_bytes);
        assert_eq!(inserted_file.md5_hash, file_info.hash_md5);
        assert_eq!(inserted_file.sha256_hash, file_info.hash_sha256);

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), 1);

        let inserted_path = inserted_paths
            .first()
            .expect("The table must contains exactly one path.");

        assert_eq!(inserted_path.file_id, inserted_file.id);
        assert_eq!(inserted_path.path, "my_files/helloworld.txt");
        assert_eq!(inserted_path.valid_until, None);

        let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;
        assert!(insertion_timestamp.with_timezone(&Utc) <= Utc::now());

        let path_saved_file = database.path_files.join(file_info.hash_sha256);
        check_file_content(path_saved_file, file_content).await?;

        Ok(())
    }

    /// Test checking that saving several files without any path nor content
    /// collision works as intended.
    ///
    /// The expected behavior is that all the saved files should be added in
    /// the storage folder, each file should have a unique entry in both the
    /// paths and files tables, and all the path should be live.
    #[tokio::test]
    async fn test_save_multiple_files() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_contents: Vec<(_, &[u8])> = vec![
            ("my_files/helloworld.txt", b"hello world!"),
            ("my_files/some_file.txt", b"I'm a sample file!"),
            ("definitely_not_a_file.txt", b"I'm not a file :)"),
        ];

        let mut file_infos = Vec::with_capacity(file_contents.len());
        for (path_storage, file_content) in &file_contents {
            let (file, file_info) = create_dummy_file(file_content).await?;

            database
                .add_new_file_to_storage(file.file_path(), path_storage, file_info.clone())
                .await?;

            file_infos.push((*path_storage, *file_content, file_info));
        }

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), file_contents.len());

        let mut inserted_files = zip(inserted_files, file_infos)
            .map(|(inserted_file, (path_file, file_content, file_info))| {
                assert_eq!(inserted_file.original_file_name, file_info.original_name);
                assert_eq!(inserted_file.size, file_info.size_in_bytes);
                assert_eq!(inserted_file.md5_hash, file_info.hash_md5);
                assert_eq!(inserted_file.sha256_hash, file_info.hash_sha256);

                (
                    path_file,
                    (inserted_file.id, file_content, file_info.hash_sha256),
                )
            })
            .collect::<HashMap<_, _>>();

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), file_contents.len());

        let now = Utc::now();

        for inserted_path in inserted_paths {
            assert!(inserted_files.contains_key(&inserted_path.path.as_str()));

            let (mapped_id, file_content, mapped_hash_256) = inserted_files
                .remove(inserted_path.path.as_str())
                .expect("The key must be present in the map.");

            assert_eq!(inserted_path.file_id, mapped_id);
            assert_eq!(inserted_path.valid_until, None);

            let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;
            assert!(insertion_timestamp.with_timezone(&Utc) <= now);

            let path_saved_file = database.path_files.join(mapped_hash_256);
            check_file_content(path_saved_file, file_content).await?;
        }

        assert!(inserted_files.is_empty());

        Ok(())
    }

    /// Test checking that trying to save the same file multiple time does not
    /// create redundant copy of the file in question.
    ///
    /// The intended behavior is for the database to first check if the new
    /// file is not already present in its table (using the file sha256 as a
    /// unique identifier). In which case an extra path pointing to the
    /// existing file should be created while preserving the file table intact.
    #[tokio::test]
    async fn test_save_same_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";
        let storage_paths = ["my_files/helloworld.txt", "my_files/different/file.txt"];

        let (file, file_info) = create_dummy_file(file_content).await?;

        database
            .add_new_file_to_storage(file.file_path(), storage_paths[0], file_info.clone())
            .await?;

        database
            .add_new_file_to_storage(file.file_path(), storage_paths[1], file_info.clone())
            .await?;

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), 1);

        let inserted_file = inserted_files
            .first()
            .expect("The table must contains exactly one file.");

        assert_eq!(inserted_file.original_file_name, file_info.original_name);
        assert_eq!(inserted_file.size, file_info.size_in_bytes);
        assert_eq!(inserted_file.md5_hash, file_info.hash_md5);
        assert_eq!(inserted_file.sha256_hash, file_info.hash_sha256);

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), 2);

        let mut present_paths = HashSet::from(storage_paths);

        for inserted_path in inserted_paths {
            assert_eq!(inserted_path.file_id, inserted_file.id);
            assert_eq!(inserted_path.valid_until, None);

            let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;
            assert!(insertion_timestamp.with_timezone(&Utc) <= Utc::now());
            assert!(present_paths.remove(inserted_path.path.as_str()));
        }

        assert!(present_paths.is_empty());

        let path_saved_file = database.path_files.join(file_info.hash_sha256);
        check_file_content(path_saved_file, file_content).await?;

        Ok(())
    }

    /// Test checking that overriding a file correctly updates the path table.
    ///
    /// The expected behavior is that the old path gets invalidated at the same
    /// time the new one becomes live, effectively replacing the old path with
    /// the new one.
    #[tokio::test]
    async fn test_override_existing_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_base_content = b"hello world!";
        let file_over_content = b"evil file override >:)";

        let (file_base, file_base_info) = create_dummy_file(file_base_content).await?;
        let (file_over, file_over_info) = create_dummy_file(file_over_content).await?;

        database
            .add_new_file_to_storage(
                file_base.file_path(),
                "my_files/override_me.txt",
                file_base_info.clone(),
            )
            .await?;

        // Wait a bit before inserting the second file.
        sleep(Duration::from_millis(100)).await;

        database
            .add_new_file_to_storage(
                file_over.file_path(),
                "my_files/override_me.txt",
                file_over_info.clone(),
            )
            .await?;

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), 2);

        let mut index_base = None;
        let mut index_over = None;

        for inserted_file in inserted_files {
            if inserted_file.sha256_hash == file_base_info.hash_sha256 {
                index_base = Some(inserted_file.id);
            } else if inserted_file.sha256_hash == file_over_info.hash_sha256 {
                index_over = Some(inserted_file.id);
            }
        }

        assert!(index_base.is_some());
        assert!(index_over.is_some());

        let index_base = index_base.expect("The option cannot be None.");
        let index_over = index_over.expect("The option cannot be None.");

        assert_ne!(index_base, index_over);

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), 2);

        let now = Utc::now();

        for inserted_path in inserted_paths {
            assert_eq!(inserted_path.path, "my_files/override_me.txt");

            let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;

            if inserted_path.file_id == index_base {
                assert!(inserted_path.valid_until.is_some());

                let deletion_timestamp = DateTime::parse_from_rfc3339(
                    &inserted_path
                        .valid_until
                        .expect("The option cannot be None"),
                )?;

                assert!(insertion_timestamp < deletion_timestamp);
                assert!(deletion_timestamp.with_timezone(&Utc) < now);
            } else {
                assert!(inserted_path.valid_until.is_none());
                assert!(insertion_timestamp.with_timezone(&Utc) < now);
            }
        }

        Ok(())
    }

    /// Test checking that trying to upload the same file to the same path does
    /// not modify the database at all.
    ///
    /// The expected behavior is for the path table to not be updated at all.
    /// We want to avoid having to path pointing to the same file where on is
    /// being invalidated at the exact same moment the other one becomes live.
    #[tokio::test]
    async fn test_override_with_same_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";

        let (file, file_info) = create_dummy_file(file_content).await?;

        database
            .add_new_file_to_storage(
                file.file_path(),
                "my_files/helloworld.txt",
                file_info.clone(),
            )
            .await?;

        // Wait a bit before inserting the second file.
        sleep(Duration::from_millis(100)).await;

        database
            .add_new_file_to_storage(
                file.file_path(),
                "my_files/helloworld.txt",
                file_info.clone(),
            )
            .await?;

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), 1);

        let inserted_file = inserted_files
            .first()
            .expect("The table must contains exactly one file.");

        assert_eq!(inserted_file.original_file_name, file_info.original_name);
        assert_eq!(inserted_file.size, file_info.size_in_bytes);
        assert_eq!(inserted_file.md5_hash, file_info.hash_md5);
        assert_eq!(inserted_file.sha256_hash, file_info.hash_sha256);

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), 1);

        let inserted_path = inserted_paths
            .first()
            .expect("The table must contains exactly one path.");

        assert_eq!(inserted_path.file_id, inserted_file.id);
        assert_eq!(inserted_path.path, "my_files/helloworld.txt");
        assert_eq!(inserted_path.valid_until, None);

        let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;
        assert!(insertion_timestamp.with_timezone(&Utc) <= Utc::now());

        let path_saved_file = database.path_files.join(file_info.hash_sha256);
        check_file_content(path_saved_file, file_content).await?;

        Ok(())
    }

    /// Test checking that saving a file already present in the storage FS but
    /// not registered in the database fixes itself.
    ///
    /// This test is for completeness, this edge case should never occur unless
    /// the database got corrupted (in which case, doing a full database sanity
    /// check would be the best course of action). In such scenario, the
    /// database is supposed to fix the file table by itself by adding an entry
    /// for the missing file.
    #[tokio::test]
    async fn test_save_existing_file_missing_from_database() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";

        let (file, file_info) = create_dummy_file(file_content).await?;

        let path_saved_file = database.path_files.join(&file_info.hash_sha256);
        File::open(path_saved_file)
            .await?
            .write_all(file_content)
            .await?;

        database
            .add_new_file_to_storage(
                file.file_path(),
                "my_files/helloworld.txt",
                file_info.clone(),
            )
            .await?;

        let inserted_files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_files.len(), 1);

        let inserted_file = inserted_files
            .first()
            .expect("The table must contains exactly one file.");

        assert_eq!(inserted_file.original_file_name, file_info.original_name);
        assert_eq!(inserted_file.size, file_info.size_in_bytes);
        assert_eq!(inserted_file.md5_hash, file_info.hash_md5);
        assert_eq!(inserted_file.sha256_hash, file_info.hash_sha256);

        let inserted_paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(inserted_paths.len(), 1);

        let inserted_path = inserted_paths
            .first()
            .expect("The table must contains exactly one path.");

        assert_eq!(inserted_path.file_id, inserted_file.id);
        assert_eq!(inserted_path.path, "my_files/helloworld.txt");
        assert_eq!(inserted_path.valid_until, None);

        let insertion_timestamp = DateTime::parse_from_rfc3339(&inserted_path.valid_since)?;
        assert!(insertion_timestamp.with_timezone(&Utc) <= Utc::now());

        Ok(())
    }
}
