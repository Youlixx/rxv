use std::path::Path;

use chrono::Utc;
use tokio::fs;

use crate::{
    path::StoragePath,
    response::{Error, Result},
};

use super::AppState;

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
        path_storage: impl Into<StoragePath>,
        file_info: FileInfo,
    ) -> Result<()> {
        let path_storage: StoragePath = path_storage.into();

        if !path_storage.is_file() {
            return Err(Error::InvalidFilePath(path_storage));
        }

        let path_copy = self.path_files.join(&file_info.hash_sha256);
        let path_storage = path_storage.to_str();
        let timestamp = Utc::now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        let current_file_id = sqlx::query!(
            r#"SELECT id as "id!" FROM files WHERE sha256_hash = ?;"#,
            file_info.hash_sha256
        )
        .fetch_one(&mut *transaction)
        .await;

        let file_id = match current_file_id {
            Ok(record) => record.id,
            Err(error) => match error {
                sqlx::Error::RowNotFound => {
                    if !fs::try_exists(&path_copy).await? {
                        fs::copy(path_file, path_copy).await?;
                    }

                    sqlx::query!(
                        r#"
                        INSERT INTO files (original_file_name, size, md5_hash, sha256_hash, upload_date)
                        VALUES (?, ?, ?, ?, ?)
                        "#,
                        file_info.original_name,
                        file_info.size_in_bytes,
                        file_info.hash_md5,
                        file_info.hash_sha256,
                        timestamp
                    )
                    .execute(&mut *transaction)
                    .await?
                    .last_insert_rowid() as i64
                }
                _ => return Err(error.into()),
            },
        };

        let same_file_with_same_path = sqlx::query!(
            r#"
            SELECT COUNT(id) as count FROM paths
            WHERE valid_until IS NULL AND path == ? AND file_id = ?;
            "#,
            path_storage,
            file_id
        )
        .fetch_one(&mut *transaction)
        .await?
        .count;

        if same_file_with_same_path > 0 {
            transaction.rollback().await?;
            return Ok(());
        }

        sqlx::query!(
            r#"
            UPDATE paths
            SET valid_until = ?
            WHERE path = ? AND valid_until IS NULL;
            "#,
            timestamp,
            path_storage
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query!(
            r#"
            INSERT INTO paths (file_id, path, valid_since, valid_until)
            VALUES (?, ?, ?, NULL);
            "#,
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
        database::AppState,
        response::{Error, Result},
    };

    use super::FileInfo;

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

    /// Test to verify that saving a single file to the storage works as
    /// intended.
    ///
    /// The expected behavior is that both the paths and files tables contain
    /// exactly one entry for the new file. The file should also be present in
    /// the storage folder.
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

    /// Test to verify that saving several files without any path or content
    /// collisions works as intended.
    ///
    /// The expected behavior is that all saved files are added to the storage
    /// folder, each file has a unique entry in both the paths and files tables,
    /// and all paths are marked as live.
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

    /// Test to verify that saving the same file multiple times does not create
    /// redundant copies of the file.
    ///
    /// The expected behavior is that the database first checks if the new file
    /// is already present in its table using the file's SHA-256 hash as a
    /// unique identifier. If the file is already present, an additional path
    /// pointing to the existing file should be created, while the file table
    /// remains unchanged.
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

    /// Test to verify that overriding a file correctly updates the path table.
    ///
    /// The expected behavior is that the old path is invalidated at the same
    /// time the new path becomes live, effectively replacing the old path with
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

    /// Test to verify that uploading the same file to the same path does not
    /// modify the database.
    ///
    /// The expected behavior is that the path table remains unchanged. This
    /// avoids having two paths pointing to the same file, where one is
    /// invalidated at the exact moment the other becomes live.
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

    /// Test to verify that saving a file already present in the storage
    /// filesystem but not registered in the database corrects the issue.
    ///
    /// This test covers an edge case that should never occur unless the
    /// database becomes corrupted. In such a scenario, the best course of
    /// action would be to perform a full database sanity check. The database
    /// should automatically fix the file table by adding an entry for the
    /// missing file.
    #[tokio::test]
    async fn test_save_existing_file_missing_from_database() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";

        let (file, file_info) = create_dummy_file(file_content).await?;

        let path_saved_file = database.path_files.join(&file_info.hash_sha256);
        File::create(path_saved_file)
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

    /// Test to verify that attempting to save a file with an invalid path
    /// fails.
    ///
    /// The expected behavior is that an error is returned when the given path
    /// points to the root or a folder instead of a file.
    #[tokio::test]
    async fn test_save_with_invalid_path() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let file_content = b"hello world!";

        let (file, file_info) = create_dummy_file(file_content).await?;

        let insert_to_root_result = database
            .add_new_file_to_storage(file.file_path(), "", file_info.clone())
            .await;

        assert!(insert_to_root_result.is_err());
        assert!(matches!(
            insert_to_root_result
                .err()
                .expect("The result must be an error."),
            Error::InvalidFilePath { .. }
        ));

        let insert_to_directory_result = database
            .add_new_file_to_storage(file.file_path(), "path/to/some/folder/", file_info.clone())
            .await;

        assert!(insert_to_directory_result.is_err());
        assert!(matches!(
            insert_to_directory_result
                .err()
                .expect("The result must be an error."),
            Error::InvalidFilePath { .. }
        ));

        let dir_entry = fs::read_dir(database.path_files)
            .await?
            .next_entry()
            .await?;

        assert!(dir_entry.is_none());

        Ok(())
    }
}
