use chrono::Utc;

use crate::{
    path::StoragePath,
    response::{Error, Result},
};

use super::AppState;

impl AppState {
    /// Delete a file from the current storage.
    ///
    /// This function does not delete the actual files on the disk. Instead, it
    /// marks the specified storage paths as invalid, effectively removing them
    /// from the live storage.
    pub async fn delete_file_from_storage(
        &self,
        path_storage: impl Into<StoragePath>,
    ) -> Result<()> {
        let path_storage: StoragePath = path_storage.into();
        let timestamp = Utc::now().to_rfc3339();

        let files_deleted = if path_storage.is_file() {
            let path_storage = path_storage.to_str();

            sqlx::query!(
                "
                UPDATE paths
                SET valid_until = ?
                WHERE path = ? AND valid_until IS NULL;
                ",
                timestamp,
                path_storage
            )
            .execute(&self.database)
            .await?
            .rows_affected()
        } else {
            let matching_pattern = path_storage.get_sql_matching_pattern();

            sqlx::query!(
                "
                UPDATE paths
                SET valid_until = ?
                WHERE path LIKE ? AND valid_until IS NULL;
                ",
                timestamp,
                matching_pattern
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
    use async_tempfile::{TempDir, TempFile};
    use chrono::DateTime;
    use md5::{Digest, Md5};
    use sha2::Sha256;
    use tokio::io::AsyncWriteExt;

    use crate::{
        database::{upload::FileInfo, AppState},
        path::StoragePath,
        response::{Error, Result},
    };

    async fn add_file_to_storage(
        database: &AppState,
        file_content: &[u8],
        path_storage: impl Into<StoragePath>,
    ) -> Result<()> {
        let mut file = TempFile::new().await?;
        file.write(file_content).await?;

        let mut hash_md5 = Md5::new();
        hash_md5.update(file_content);

        let mut hash_sha256 = Sha256::new();
        hash_sha256.update(file_content);

        database
            .add_new_file_to_storage(
                file.file_path(),
                path_storage,
                FileInfo {
                    original_name: "some_file.txt".into(),
                    size_in_bytes: file_content.len() as i64,
                    hash_md5: hex::encode(hash_md5.finalize()),
                    hash_sha256: hex::encode(hash_sha256.finalize()),
                },
            )
            .await?;

        Ok(())
    }

    /// Test to verify that deleting a single file works as intended.
    ///
    /// The expected behavior is that the existing path is marked as not live by
    /// setting an end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_single_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let path_storage = "my_files/helloworld.txt";
        add_file_to_storage(&database, b"hello world!", path_storage).await?;
        database.delete_file_from_storage(path_storage).await?;

        let files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(files.len(), 1);

        let file_id = files.first().expect("The option cannot be None.").id;
        let paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(paths.len(), 1);

        let path = paths.first().expect("The option cannot be None.");
        assert_eq!(path.file_id, file_id);
        assert!(path.valid_until.is_some());

        let insertion_timestamp = DateTime::parse_from_rfc3339(&path.valid_since)?;
        let deletion_timestamp = path
            .valid_until
            .clone()
            .expect("The option cannot be None.");
        let deletion_timestamp = DateTime::parse_from_rfc3339(&deletion_timestamp)?;

        assert!(insertion_timestamp <= deletion_timestamp);

        Ok(())
    }

    /// Test to verify that deleting a directory works as intended.
    ///
    /// The expected behavior is that all files within the directory, including
    /// those in subdirectories, have their paths marked as not live by setting
    /// an end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_directory() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_contents: Vec<(_, &[u8])> = vec![
            ("my_files/helloworld.txt", b"hello world!"),
            ("my_files/nested/some_file.txt", b"I'm a sample file!"),
            ("definitely_not_a_file.txt", b"I'm not a file :)"),
        ];

        for (path_storage, file_content) in file_contents {
            add_file_to_storage(&database, file_content, path_storage).await?;
        }

        database.delete_file_from_storage("my_files/").await?;

        let files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(files.len(), 3);

        let mut total_delete_paths = 0;
        let paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(paths.len(), 3);

        for path in paths {
            if path.path.starts_with("my_files/") {
                total_delete_paths += 1;

                assert!(path.valid_until.is_some());

                let insertion_timestamp = DateTime::parse_from_rfc3339(&path.valid_since)?;
                let deletion_timestamp = path
                    .valid_until
                    .clone()
                    .expect("The option cannot be None.");
                let deletion_timestamp = DateTime::parse_from_rfc3339(&deletion_timestamp)?;

                assert!(insertion_timestamp <= deletion_timestamp);
            } else {
                assert!(path.valid_until.is_none());
            }
        }

        assert_eq!(total_delete_paths, 2);

        Ok(())
    }

    /// Test to verify that deleting the root directory works as intended.
    ///
    /// The expected behavior is that all live files have their paths marked as
    /// not live by setting an end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_root() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        let file_contents: Vec<(_, &[u8])> = vec![
            ("my_files/helloworld.txt", b"hello world!"),
            ("my_files/nested/some_file.txt", b"I'm a sample file!"),
            ("definitely_not_a_file.txt", b"I'm not a file :)"),
        ];

        for (path_storage, file_content) in file_contents {
            add_file_to_storage(&database, file_content, path_storage).await?;
        }

        database.delete_file_from_storage("").await?;

        let files = sqlx::query!("SELECT * FROM files;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(files.len(), 3);

        let paths = sqlx::query!("SELECT * FROM paths;")
            .fetch_all(&database.database)
            .await?;

        assert_eq!(paths.len(), 3);

        for path in paths {
            assert!(path.valid_until.is_some());

            let insertion_timestamp = DateTime::parse_from_rfc3339(&path.valid_since)?;
            let deletion_timestamp = path
                .valid_until
                .clone()
                .expect("The option cannot be None.");
            let deletion_timestamp = DateTime::parse_from_rfc3339(&deletion_timestamp)?;

            assert!(insertion_timestamp <= deletion_timestamp);
        }

        Ok(())
    }

    /// Test to verify that attempting to delete a file with no currently live
    /// path fails.
    #[tokio::test]
    async fn test_delete_invalid_file() -> Result<()> {
        let test_dir = TempDir::new().await?;
        let database = AppState::new(test_dir.dir_path()).await?;

        add_file_to_storage(&database, b"hello world!", "my_files/nested/helloworld.txt").await?;

        let path_storage = "some_directory/";
        let error = database.delete_file_from_storage(path_storage).await;

        assert!(error.is_err());

        if let Err(Error::FileNotFound(path)) = error {
            assert_eq!(path.to_str(), path_storage);
        } else {
            panic!("The function returned the wrong error variant.");
        }

        database.delete_file_from_storage("my_files/nested/").await?;

        let path_storage = "my_files/nested/helloworld.txt";
        let error = database.delete_file_from_storage(path_storage).await;

        assert!(error.is_err());

        if let Err(Error::FileNotFound(path)) = error {
            assert_eq!(path.to_str(), path_storage);
        } else {
            panic!("The function returned the wrong error variant.");
        }

        let path_storage = "my_files/";
        let error = database.delete_file_from_storage(path_storage).await;

        assert!(error.is_err());

        if let Err(Error::FileNotFound(path)) = error {
            assert_eq!(path.to_str(), path_storage);
        } else {
            panic!("The function returned the wrong error variant.");
        }

        Ok(())
    }
}
