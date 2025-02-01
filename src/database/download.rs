use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::{path::StoragePath, response::Result};

use super::AppState;

#[derive(Debug)]
pub enum StoragePaths {
    None,
    File(PathBuf),
    Directory(Vec<(PathBuf, StoragePath)>),
}

impl From<Vec<(PathBuf, StoragePath)>> for StoragePaths {
    fn from(value: Vec<(PathBuf, StoragePath)>) -> Self {
        if value.is_empty() {
            StoragePaths::None
        } else {
            StoragePaths::Directory(value)
        }
    }
}

impl From<Option<PathBuf>> for StoragePaths {
    fn from(value: Option<PathBuf>) -> Self {
        match value {
            Some(path) => StoragePaths::File(path),
            None => StoragePaths::None,
        }
    }
}

impl AppState {
    /// Retrieve the list of live files that match the given base path.
    ///
    /// This function can be used to retrieve the actual physical paths to the
    /// files on the disk. It handles three cases:
    /// - If the target storage path is the actual root path, all live storage
    ///   files are returned.
    /// - If the target storage path points to a single file, the actual path to
    ///   the file is returned if it exists.
    /// - If the target storage path points to a folder (must end with '/'), all
    ///   paths to the live files within that folder are returned.
    ///
    /// Note that when dealing with the root path or folders, this function
    /// recursively retrieves all files, including those in sub-folders.
    pub async fn get_file_paths(
        &self,
        base_path: &StoragePath,
        timestamp: DateTime<Utc>,
    ) -> Result<StoragePaths> {
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf, time::Duration};

    use async_tempfile::{TempDir, TempFile};
    use chrono::Utc;
    use md5::{Digest, Md5};
    use sha2::Sha256;
    use tokio::{io::AsyncWriteExt, time::sleep};

    use crate::{
        database::{download::StoragePaths, upload::FileInfo, AppState},
        path::StoragePath,
        response::Result,
    };

    async fn add_file_to_storage(
        database: &AppState,
        file_content: &[u8],
        path_storage: &StoragePath,
    ) -> Result<PathBuf> {
        let mut file = TempFile::new().await?;
        file.write(file_content).await?;

        let mut hash_md5 = Md5::new();
        hash_md5.update(file_content);

        let mut hash_sha256 = Sha256::new();
        hash_sha256.update(file_content);
        let hash_sha256 = hex::encode(hash_sha256.finalize());

        database
            .add_new_file_to_storage(
                file.file_path(),
                path_storage,
                FileInfo {
                    original_name: "some_file.txt".into(),
                    size_in_bytes: file_content.len() as i64,
                    hash_md5: hex::encode(hash_md5.finalize()),
                    hash_sha256: hash_sha256.clone(),
                },
            )
            .await?;

        Ok(database.path_files.join(hash_sha256))
    }

    /// Test to verify that getting a single file returns the expected path.
    ///
    /// The expected behavior is that the function returns a
    /// [`StoragePaths::File`] with a path to the actual file on the server
    /// disk.
    #[tokio::test]
    async fn test_get_single_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let path_storage = StoragePath::from("my_files/helloworld.txt");
        let path_file = add_file_to_storage(&database, b"hello world!", &path_storage).await?;

        let paths = database.get_file_paths(&path_storage, Utc::now()).await?;

        assert!(matches!(paths, StoragePaths::File { .. }));

        if let StoragePaths::File(path) = paths {
            assert_eq!(path, path_file);
        } else {
            unreachable!();
        }

        Ok(())
    }

    /// Test to verify that getting a directory returns the expected paths.
    ///
    /// The expected behavior is that the function returns a
    /// [`StoragePaths::Directory`] where each entry is a tuple of the path to
    /// the physical disk on the server side and the corresponding storage path.
    #[tokio::test]
    async fn test_get_directory_files() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_contents: Vec<(_, &[u8])> = vec![
            ("my_files/helloworld.txt", b"hello world!"),
            ("my_files/some_file.txt", b"I'm a sample file!"),
            ("definitely_not_a_file.txt", b"I'm not a file :)"),
        ];

        let mut file_paths = HashMap::new();
        for (path_storage, file_content) in file_contents {
            let path_storage = StoragePath::from(path_storage);
            let path_file = add_file_to_storage(&database, file_content, &path_storage).await?;
            file_paths.insert(path_storage, path_file);
        }

        let paths = database
            .get_file_paths(&StoragePath::from("my_files/"), Utc::now())
            .await?;

        assert!(matches!(paths, StoragePaths::Directory { .. }));

        if let StoragePaths::Directory(paths) = paths {
            assert_eq!(paths.len(), 2);

            paths.into_iter().for_each(|(path_file, path_storage)| {
                let expected_path = file_paths.remove(&path_storage);

                assert!(expected_path.is_some());
                assert_eq!(
                    expected_path.expect("The option cannot be None."),
                    path_file
                );
            });
        } else {
            unreachable!();
        }

        Ok(())
    }

    /// Test to verify that getting a directory with a single file returns the
    /// expected paths.
    ///
    /// The expected behavior is that, even though the folder contains only a
    /// single file, the function should still return
    /// [`StoragePaths::Directory`] since we are querying for a directory. The
    /// path list should contain a single entry mapping to the file.
    #[tokio::test]
    async fn test_get_directory_single_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let path_storage = StoragePath::from("my_files/helloworld.txt");
        let path_file = add_file_to_storage(&database, b"hello world!", &path_storage).await?;

        let paths = database
            .get_file_paths(&StoragePath::from("my_files/"), Utc::now())
            .await?;

        assert!(matches!(paths, StoragePaths::Directory { .. }));

        if let StoragePaths::Directory(paths) = paths {
            assert_eq!(paths.len(), 1);

            let path = paths.first().expect("The option cannot be None.");
            assert_eq!(path.0, path_file);
            assert_eq!(path.1, path_storage);
        } else {
            unreachable!();
        }

        Ok(())
    }

    /// Test to verify that requesting the root returns all currently live paths
    /// from the storage.
    ///
    /// The expected behavior is that the function returns a
    /// [`StoragePaths::Directory`] containing all storage files.
    #[tokio::test]
    async fn test_get_all_files() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_contents: Vec<(_, &[u8])> = vec![
            ("my_files/helloworld.txt", b"hello world!"),
            ("my_files/some_file.txt", b"I'm a sample file!"),
            ("definitely_not_a_file.txt", b"I'm not a file :)"),
        ];

        let mut file_paths = HashMap::new();
        for (path_storage, file_content) in &file_contents {
            let path_storage = StoragePath::from(path_storage);
            let path_file = add_file_to_storage(&database, file_content, &path_storage).await?;
            file_paths.insert(path_storage, path_file);
        }

        let paths = database
            .get_file_paths(&StoragePath::from(""), Utc::now())
            .await?;

        assert!(matches!(paths, StoragePaths::Directory { .. }));

        if let StoragePaths::Directory(paths) = paths {
            assert_eq!(paths.len(), file_contents.len());

            paths.into_iter().for_each(|(path_file, path_storage)| {
                let expected_path = file_paths.remove(&path_storage);

                assert!(expected_path.is_some());
                assert_eq!(
                    expected_path.expect("The option cannot be None."),
                    path_file
                );
            });
        } else {
            unreachable!();
        }

        Ok(())
    }

    /// Test to verify that requesting a non-existent file returns nothing.
    ///
    /// The expected behavior is that, whether requesting a file or a directory,
    /// the function should return [`StoragePaths::None`] in both cases.
    #[tokio::test]
    async fn test_get_invalid_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let path_storage = StoragePath::from("my_files/helloworld.txt");
        add_file_to_storage(&database, b"hello world!", &path_storage).await?;

        let paths = database
            .get_file_paths(&StoragePath::from("some_directory/"), Utc::now())
            .await?;

        assert!(matches!(paths, StoragePaths::None));

        let paths = database
            .get_file_paths(&StoragePath::from("some_file.txt"), Utc::now())
            .await?;

        assert!(matches!(paths, StoragePaths::None));

        Ok(())
    }

    /// Test to verify that requesting a single file returns the file that was
    /// live at the specified timestamp.
    ///
    /// The expected behavior is that if the given timestamp is before the file
    /// was inserted, [`StoragePaths::None`] should be returned. If a live file
    /// exists at the given timestamp, [`StoragePaths::File`] with the path to
    /// the live file should be returned.
    #[tokio::test]
    async fn test_get_file_with_timestamp() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let path_storage = StoragePath::from("my_files/helloworld.txt");

        let time_stamp_0 = Utc::now();
        sleep(Duration::from_millis(10)).await;

        let path_file_0 = add_file_to_storage(&database, b"hello world!", &path_storage).await?;

        let time_stamp_1 = Utc::now();
        sleep(Duration::from_millis(10)).await;

        let path_file_1 = add_file_to_storage(&database, b"different file!", &path_storage).await?;

        let paths = database.get_file_paths(&path_storage, time_stamp_0).await?;
        assert!(matches!(paths, StoragePaths::None));

        let paths = database.get_file_paths(&path_storage, time_stamp_1).await?;
        assert!(matches!(paths, StoragePaths::File { .. }));

        if let StoragePaths::File(path) = paths {
            assert_eq!(path, path_file_0);
        } else {
            unreachable!();
        }

        let paths = database.get_file_paths(&path_storage, Utc::now()).await?;
        assert!(matches!(paths, StoragePaths::File { .. }));

        if let StoragePaths::File(path) = paths {
            assert_eq!(path, path_file_1);
        } else {
            unreachable!();
        }

        Ok(())
    }

    /// Test to verify that requesting multiple files with a timestamp only
    /// returns the files that were live at that timestamp.
    ///
    /// The expected behavior is that if the given timestamp is before any file
    /// was inserted, [`StoragePaths::None`] should be returned. If some live
    /// files exist at the given timestamp, [`StoragePaths::Directory`] with the
    /// paths to the currently live files should be returned.
    #[tokio::test]
    async fn test_get_directory_with_timestamp() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;
        let path_storage = StoragePath::from("my_files/");

        let time_stamp_0 = Utc::now();
        sleep(Duration::from_millis(10)).await;

        let path_storage_0 = StoragePath::from("my_files/helloworld.txt");
        let path_file_0 = add_file_to_storage(&database, b"hello world!", &path_storage_0).await?;

        let time_stamp_1 = Utc::now();
        sleep(Duration::from_millis(10)).await;

        let path_storage_1 = StoragePath::from("my_files/another_file.txt");
        let path_file_1 =
            add_file_to_storage(&database, b"different file!", &path_storage_1).await?;

        let paths = database.get_file_paths(&path_storage, time_stamp_0).await?;
        assert!(matches!(paths, StoragePaths::None));

        let paths = database.get_file_paths(&path_storage, time_stamp_1).await?;
        assert!(matches!(paths, StoragePaths::Directory { .. }));

        if let StoragePaths::Directory(paths) = paths {
            assert_eq!(paths.len(), 1);

            let path = paths.first().expect("The option cannot be None.");
            assert_eq!(path.0, path_file_0);
            assert_eq!(path.1, path_storage_0);
        } else {
            unreachable!();
        }

        let paths = database.get_file_paths(&path_storage, Utc::now()).await?;
        assert!(matches!(paths, StoragePaths::Directory { .. }));

        if let StoragePaths::Directory(mut paths) = paths {
            assert_eq!(paths.len(), 2);

            let path_0 = paths.pop().expect("The option cannot be None");
            let path_1 = paths.pop().expect("The option cannot be None");

            // There is no guarantee on the return order.
            let (path_0, path_1) = if path_0.0 == path_file_1 {
                (path_1, path_0)
            } else {
                (path_0, path_1)
            };

            assert_eq!(path_0.0, path_file_0);
            assert_eq!(path_0.1, path_storage_0);

            assert_eq!(path_1.0, path_file_1);
            assert_eq!(path_1.1, path_storage_1);
        } else {
            unreachable!();
        }

        Ok(())
    }
}
