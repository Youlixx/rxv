use chrono::{DateTime, Utc};

use super::{
    FileDatabase, TimeProvider,
    error::{Error, Result},
    save_file::FileMetadata,
    virtual_path::VirtualPath,
};

#[derive(Debug, Eq, PartialEq)]
pub struct PathMetadataPair {
    pub virtual_path: VirtualPath,
    pub metadata: FileMetadata,
    pub upload_timestamp: DateTime<Utc>,
}

impl<T: TimeProvider> FileDatabase<T> {
    /// Retrieve the metadata of a single file.
    pub async fn get_file_metadata(
        &self,
        virtual_path: VirtualPath,
        timestamp: DateTime<Utc>,
    ) -> Result<PathMetadataPair> {
        if !virtual_path.is_file() {
            return Err(Error::NotAVirtualFile(virtual_path));
        }

        let timestamp_str = timestamp.to_rfc3339();
        let path_storage = virtual_path.path();

        let file = sqlx::query!(
            r#"
            SELECT
                files.original_file_name,
                files.size,
                files.hash,
                files.upload_date
            FROM files
            INNER JOIN paths ON files.id == paths.file_id
            WHERE ? >= paths.valid_since
                AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                AND paths.path == ?;
            "#,
            timestamp_str,
            timestamp_str,
            path_storage
        )
        .fetch_optional(&self.database)
        .await?
        .ok_or(Error::VirtualFileNotFound(virtual_path.clone()))?;

        Ok(PathMetadataPair {
            virtual_path,
            metadata: FileMetadata {
                original_file_name: file.original_file_name,
                size_in_bytes: file.size as usize,
                hash: file.hash,
            },
            upload_timestamp: DateTime::parse_from_rfc3339(&file.upload_date)?.to_utc(),
        })
    }

    /// Retrieve the metadata of all the files contained in the given directory.
    pub async fn get_tree_metadata(
        &self,
        virtual_path: impl Into<VirtualPath>,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<PathMetadataPair>> {
        let virtual_path: VirtualPath = virtual_path.into();

        if !virtual_path.is_dir() {
            return Err(Error::NotAVirtualDirectory(virtual_path));
        }

        let timestamp_str = timestamp.to_rfc3339();
        let path_wildcard = virtual_path.match_pattern();

        let files = sqlx::query!(
            r#"
            SELECT
                files.original_file_name,
                files.size,
                files.hash,
                files.upload_date,
                paths.path
            FROM files
            INNER JOIN paths ON files.id == paths.file_id
            WHERE ? >= paths.valid_since
                AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                AND paths.path LIKE ?;
            "#,
            timestamp_str,
            timestamp_str,
            path_wildcard
        )
        .fetch_all(&self.database)
        .await?;

        if files.is_empty() {
            if virtual_path.is_root() {
                return Ok(Vec::new());
            } else {
                return Err(Error::VirtualFileNotFound(virtual_path));
            }
        }

        Ok(files
            .into_iter()
            .map(|file| -> Result<PathMetadataPair> {
                Ok(PathMetadataPair {
                    virtual_path: VirtualPath::from(file.path),
                    metadata: FileMetadata {
                        original_file_name: file.original_file_name,
                        size_in_bytes: file.size as usize,
                        hash: file.hash,
                    },
                    upload_timestamp: DateTime::parse_from_rfc3339(&file.upload_date)?.to_utc(),
                })
            })
            .collect::<Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{
        TimeProvider,
        error::{Error, Result},
        get_metadata::PathMetadataPair,
        save_file::FileMetadata,
        tests::{FileOperation, TestDatabase, get_hash, get_timestamp, setup_test_database},
        virtual_path::VirtualPath,
    };

    impl TestDatabase {
        async fn get_file_metadata_at_internal_timestamp(
            &self,
            virtual_path: VirtualPath,
        ) -> Result<PathMetadataPair> {
            self.get_file_metadata(virtual_path, self.time_provider.now())
                .await
        }

        async fn get_tree_metadata_at_internal_timestamp(
            &self,
            virtual_path: impl Into<VirtualPath>,
        ) -> Result<Vec<PathMetadataPair>> {
            self.get_tree_metadata(virtual_path, self.time_provider.now())
                .await
        }
    }

    /// Test to verify that getting a single file metadata returns the expected result.
    #[tokio::test]
    async fn test_get_file_metadata() -> Result<()> {
        let path = VirtualPath::from("/my_files/helloworld.txt");
        let original_file_name = "some_file.txt";
        let content = b"hello world!".to_vec().into_boxed_slice();

        let (_test_dir, database) = setup_test_database(vec![FileOperation::Save {
            original_file_name: original_file_name.to_owned(),
            virtual_path: path.clone(),
            content: content.clone(),
        }])
        .await?;

        assert_eq!(
            database
                .get_file_metadata_at_internal_timestamp(path.clone())
                .await?,
            PathMetadataPair {
                virtual_path: path,
                metadata: FileMetadata {
                    original_file_name: original_file_name.to_owned(),
                    size_in_bytes: content.len(),
                    hash: get_hash(&content)
                },
                upload_timestamp: get_timestamp(0)
            }
        );

        Ok(())
    }

    /// Test to verify that trying to get the metadata of a non-existing file fails.
    #[tokio::test]
    async fn test_get_missing_file_metadata() -> Result<()> {
        let path = VirtualPath::from("/my_files/helloworld.txt");
        let (_test_dir, database) = setup_test_database(vec![]).await?;

        let get_metadata_result = database
            .get_file_metadata_at_internal_timestamp(path.clone())
            .await;

        assert!(get_metadata_result.is_err());

        match get_metadata_result.err().unwrap() {
            Error::VirtualFileNotFound(error_path) => assert_eq!(path, error_path),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that trying to get the metadata of a directory fails.
    #[tokio::test]
    async fn test_get_file_metadata_directory() -> Result<()> {
        let path = VirtualPath::from("/some/directory/");
        let (_test_dir, database) = setup_test_database(vec![]).await?;

        let get_metadata_result = database
            .get_file_metadata_at_internal_timestamp(path.clone())
            .await;

        assert!(get_metadata_result.is_err());

        match get_metadata_result.err().unwrap() {
            Error::NotAVirtualFile(error_path) => assert_eq!(path, error_path),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that trying to get the metadata of a non-live file fails.
    #[tokio::test]
    async fn test_get_missing_file_metadata_timestamp() -> Result<()> {
        let path = VirtualPath::from("/my_files/helloworld.txt");
        let original_file_name = "some_file.txt";
        let content = b"hello world!".to_vec().into_boxed_slice();

        let (_test_dir, database) = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "dummy.txt".to_owned(),
                virtual_path: VirtualPath::from("dummy.txt"),
                content: b"dummy".to_vec().into_boxed_slice(),
            },
            FileOperation::Save {
                original_file_name: original_file_name.to_owned(),
                virtual_path: path.clone(),
                content: content.clone(),
            },
        ])
        .await?;

        let get_metadata_result = database
            .get_file_metadata(path.clone(), get_timestamp(0))
            .await;

        assert!(get_metadata_result.is_err());

        match get_metadata_result.err().unwrap() {
            Error::VirtualFileNotFound(error_path) => assert_eq!(path, error_path),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that getting a directory tree returns the expected result.
    #[tokio::test]
    async fn test_get_tree_metadata() -> Result<()> {
        let files: [(_, _, &[u8]); 3] = [
            ("file1.txt", "my_files/helloworld.txt", b"hello world!"),
            ("file2.txt", "my_files/some_file.txt", b"I'm a sample file!"),
            (
                "file3.txt",
                "definitely_not_a_file.txt",
                b"I'm not a file :)",
            ),
        ];

        let (_test_dir, database) = setup_test_database(
            files
                .iter()
                .map(|(filename, virtual_path, content)| FileOperation::Save {
                    original_file_name: filename.to_string(),
                    virtual_path: VirtualPath::from(virtual_path),
                    content: content.to_vec().into_boxed_slice(),
                })
                .collect(),
        )
        .await?;

        let expected_file_metadata = files
            .into_iter()
            .enumerate()
            .map(
                |(index, (filename, virtual_path, content))| PathMetadataPair {
                    virtual_path: VirtualPath::from(virtual_path),
                    metadata: FileMetadata {
                        original_file_name: filename.to_string(),
                        size_in_bytes: content.len(),
                        hash: get_hash(content),
                    },
                    upload_timestamp: get_timestamp(index),
                },
            )
            .collect::<Vec<_>>();

        assert_eq!(
            database
                .get_tree_metadata_at_internal_timestamp(VirtualPath::from("/my_files/"))
                .await?,
            expected_file_metadata[..2]
        );

        assert_eq!(
            database
                .get_tree_metadata_at_internal_timestamp(VirtualPath::default())
                .await?,
            expected_file_metadata
        );

        Ok(())
    }

    /// Test to verify that trying to get the tree of a non-existing directory fails.
    #[tokio::test]
    async fn test_get_missing_directory_metadata() -> Result<()> {
        let path = VirtualPath::from("/my_files/");
        let (_test_dir, database) = setup_test_database(vec![]).await?;

        let get_metadata_result = database
            .get_tree_metadata_at_internal_timestamp(path.clone())
            .await;

        assert!(get_metadata_result.is_err());

        match get_metadata_result.err().unwrap() {
            Error::VirtualFileNotFound(error_path) => assert_eq!(path, error_path),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that trying to get the tree of the empty root returns an empty
    /// file list.
    #[tokio::test]
    async fn test_get_empty_root_metadata() -> Result<()> {
        let (_test_dir, database) = setup_test_database(vec![]).await?;

        let get_metadata_result = database
            .get_tree_metadata_at_internal_timestamp(VirtualPath::default())
            .await?;

        assert!(get_metadata_result.is_empty());

        Ok(())
    }

    /// Test to verify that trying to get the tree of a file fails.
    #[tokio::test]
    async fn test_get_tree_metadata_file() -> Result<()> {
        let path = VirtualPath::from("/some/file");
        let (_test_dir, database) = setup_test_database(vec![]).await?;

        let get_metadata_result = database
            .get_tree_metadata_at_internal_timestamp(path.clone())
            .await;

        assert!(get_metadata_result.is_err());

        match get_metadata_result.err().unwrap() {
            Error::NotAVirtualDirectory(error_path) => assert_eq!(path, error_path),
            _ => assert!(false),
        }

        Ok(())
    }
}
