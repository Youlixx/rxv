use chrono::{DateTime, Utc};

use super::{
    FileDatabase,
    error::{Error, InternalError, Result},
    virtual_path::VirtualPath,
};

impl FileDatabase {
    /// Delete a file from the current storage.
    ///
    /// This function does not delete the actual files on the disk. Instead, it
    /// marks the specified storage paths as invalid, effectively removing them
    /// from the live storage.
    pub async fn delete_file(
        &self,
        virtual_path: impl Into<VirtualPath>,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let virtual_path: VirtualPath = virtual_path.into();
        let timestamp_str = timestamp.to_rfc3339();
        let mut transaction = self.database.begin().await?;

        if virtual_path.is_file() {
            let path_storage = virtual_path.path();

            let last_file_timestamp = sqlx::query!(
                r#"
                SELECT valid_since as "valid_since!" FROM paths
                WHERE path = ? AND valid_until IS NULL;
                "#,
                path_storage
            )
            .fetch_optional(&mut *transaction)
            .await?;

            match last_file_timestamp {
                None => {
                    transaction.rollback().await?;
                    return Err(Error::VirtualFileNotFound(virtual_path));
                }
                Some(last_file_timestamp) => {
                    let last_timestamp =
                        DateTime::parse_from_rfc3339(&last_file_timestamp.valid_since)?.to_utc();

                    if last_timestamp > timestamp {
                        transaction.rollback().await?;

                        return Err(InternalError::InconsistentTimestamp {
                            existing: last_timestamp,
                            inserted: timestamp,
                        }
                        .into());
                    }
                }
            }

            sqlx::query!(
                r#"
                UPDATE paths
                SET valid_until = ?
                WHERE path = ? AND valid_until IS NULL;
                "#,
                timestamp_str,
                path_storage
            )
            .execute(&mut *transaction)
            .await?;
        } else {
            let matching_pattern = virtual_path.match_pattern();

            let last_file_timestamps = sqlx::query!(
                r#"
                SELECT valid_since as "valid_since!" FROM paths
                WHERE path LIKE ? AND valid_until IS NULL;
                "#,
                matching_pattern
            )
            .fetch_all(&mut *transaction)
            .await?;

            if last_file_timestamps.is_empty() {
                transaction.rollback().await?;
                return Err(Error::VirtualFileNotFound(virtual_path));
            }

            for last_file_timestamp in last_file_timestamps {
                let last_timestamp =
                    DateTime::parse_from_rfc3339(&last_file_timestamp.valid_since)?.to_utc();

                if last_timestamp > timestamp {
                    transaction.rollback().await?;

                    return Err(InternalError::InconsistentTimestamp {
                        existing: last_timestamp,
                        inserted: timestamp,
                    }
                    .into());
                }
            }

            sqlx::query!(
                r#"
                UPDATE paths
                SET valid_until = ?
                WHERE path LIKE ? AND valid_until IS NULL;
                "#,
                timestamp_str,
                matching_pattern
            )
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{
        error::{Error, InternalError, Result},
        tests::{
            FileDatabaseFile, FileDatabasePath, FileDatabaseState, FileOperation,
            check_database_state, get_timestamp, setup_test_database,
        },
        virtual_path::VirtualPath,
    };

    /// Test to verify that deleting a single file works as intended.
    ///
    /// The expected behavior is that the existing path is marked as not live by setting
    /// an end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_single_file() -> Result<()> {
        let path = VirtualPath::from("/some/file.txt");
        let content = b"hello world!".to_vec().into_boxed_slice();

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "some_file.txt".to_owned(),
                    virtual_path: path.clone(),
                    content: content.clone(),
                    timestamp: get_timestamp(0),
                },
                FileOperation::Delete {
                    virtual_path: path.clone(),
                    timestamp: get_timestamp(1),
                },
            ],
            FileDatabaseState {
                files: vec![FileDatabaseFile {
                    original_file_name: "some_file.txt".to_owned(),
                    upload_date: get_timestamp(0),
                    content: content,
                }],
                paths: vec![FileDatabasePath {
                    file_id: 1,
                    path: path,
                    valid_since: get_timestamp(0),
                    valid_until: Some(get_timestamp(1)),
                }],
            },
        )
        .await
    }

    /// Test to verify that deleting a directory works as intended.
    ///
    /// The expected behavior is that all files within the directory, including those in
    /// subdirectories, have their paths marked as not live by setting an
    /// end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_directory() -> Result<()> {
        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "file1.txt".to_owned(),
                    virtual_path: "my_files/helloworld.txt".into(),
                    content: b"hello world!".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(0),
                },
                FileOperation::Save {
                    original_file_name: "file2.txt".to_owned(),
                    virtual_path: "my_files/some_file.txt".into(),
                    content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(1),
                },
                FileOperation::Save {
                    original_file_name: "file3.txt".to_owned(),
                    virtual_path: "definitely_not_a_file.txt".into(),
                    content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(2),
                },
                FileOperation::Delete {
                    virtual_path: "my_files/".into(),
                    timestamp: get_timestamp(3),
                },
            ],
            FileDatabaseState {
                files: vec![
                    FileDatabaseFile {
                        original_file_name: "file1.txt".to_owned(),
                        upload_date: get_timestamp(0),
                        content: b"hello world!".to_vec().into_boxed_slice(),
                    },
                    FileDatabaseFile {
                        original_file_name: "file2.txt".to_owned(),
                        upload_date: get_timestamp(1),
                        content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                    },
                    FileDatabaseFile {
                        original_file_name: "file3.txt".to_owned(),
                        upload_date: get_timestamp(2),
                        content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                    },
                ],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: "/my_files/helloworld.txt".into(),
                        valid_since: get_timestamp(0),
                        valid_until: Some(get_timestamp(3)),
                    },
                    FileDatabasePath {
                        file_id: 2,
                        path: "/my_files/some_file.txt".into(),
                        valid_since: get_timestamp(1),
                        valid_until: Some(get_timestamp(3)),
                    },
                    FileDatabasePath {
                        file_id: 3,
                        path: "/definitely_not_a_file.txt".into(),
                        valid_since: get_timestamp(2),
                        valid_until: None,
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that deleting the root directory works as intended.
    ///
    /// The expected behavior is that all live files have their paths marked as not live
    /// by setting an end-of-validity timestamp.
    #[tokio::test]
    async fn test_delete_root() -> Result<()> {
        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "file1.txt".to_owned(),
                    virtual_path: "my_files/helloworld.txt".into(),
                    content: b"hello world!".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(0),
                },
                FileOperation::Save {
                    original_file_name: "file2.txt".to_owned(),
                    virtual_path: "my_files/some_file.txt".into(),
                    content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(1),
                },
                FileOperation::Save {
                    original_file_name: "file3.txt".to_owned(),
                    virtual_path: "definitely_not_a_file.txt".into(),
                    content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                    timestamp: get_timestamp(2),
                },
                FileOperation::Delete {
                    virtual_path: "/".into(),
                    timestamp: get_timestamp(3),
                },
            ],
            FileDatabaseState {
                files: vec![
                    FileDatabaseFile {
                        original_file_name: "file1.txt".to_owned(),
                        upload_date: get_timestamp(0),
                        content: b"hello world!".to_vec().into_boxed_slice(),
                    },
                    FileDatabaseFile {
                        original_file_name: "file2.txt".to_owned(),
                        upload_date: get_timestamp(1),
                        content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                    },
                    FileDatabaseFile {
                        original_file_name: "file3.txt".to_owned(),
                        upload_date: get_timestamp(2),
                        content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                    },
                ],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: "/my_files/helloworld.txt".into(),
                        valid_since: get_timestamp(0),
                        valid_until: Some(get_timestamp(3)),
                    },
                    FileDatabasePath {
                        file_id: 2,
                        path: "/my_files/some_file.txt".into(),
                        valid_since: get_timestamp(1),
                        valid_until: Some(get_timestamp(3)),
                    },
                    FileDatabasePath {
                        file_id: 3,
                        path: "/definitely_not_a_file.txt".into(),
                        valid_since: get_timestamp(2),
                        valid_until: Some(get_timestamp(3)),
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that attempting to delete a file with no currently live path
    /// fails.
    #[tokio::test]
    async fn test_delete_invalid_file() -> Result<()> {
        let delete_file_result = setup_test_database(vec![FileOperation::Delete {
            virtual_path: "/path/to/some/file".into(),
            timestamp: get_timestamp(0),
        }])
        .await;

        assert!(delete_file_result.is_err());
        assert!(matches!(
            delete_file_result
                .err()
                .expect("The result must be an error."),
            Error::VirtualFileNotFound { .. }
        ));

        let delete_directory_result = setup_test_database(vec![FileOperation::Delete {
            virtual_path: "/path/to/some/dir/".into(),
            timestamp: get_timestamp(0),
        }])
        .await;

        assert!(delete_directory_result.is_err());
        assert!(matches!(
            delete_directory_result
                .err()
                .expect("The result must be an error."),
            Error::VirtualFileNotFound { .. }
        ));

        let delete_root_result = setup_test_database(vec![FileOperation::Delete {
            virtual_path: "/".into(),
            timestamp: get_timestamp(0),
        }])
        .await;

        assert!(delete_root_result.is_err());
        assert!(matches!(
            delete_root_result
                .err()
                .expect("The result must be an error."),
            Error::VirtualFileNotFound { .. }
        ));

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_invalid_timestamp() -> Result<()> {
        let delete_with_invalid_timestamp = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "file1.txt".to_owned(),
                virtual_path: "my_files/helloworld.txt".into(),
                content: b"hello world!".to_vec().into_boxed_slice(),
                timestamp: get_timestamp(1),
            },
            FileOperation::Delete {
                virtual_path: "/".into(),
                timestamp: get_timestamp(0),
            },
        ])
        .await;

        assert!(delete_with_invalid_timestamp.is_err());

        match delete_with_invalid_timestamp.err().unwrap() {
            Error::Internal(InternalError::InconsistentTimestamp { existing, inserted }) => {
                assert_eq!(existing, get_timestamp(1));
                assert_eq!(inserted, get_timestamp(0));
            }
            _ => assert!(false),
        };

        let delete_with_invalid_timestamp = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "file1.txt".to_owned(),
                virtual_path: "my_files/helloworld.txt".into(),
                content: b"hello world!".to_vec().into_boxed_slice(),
                timestamp: get_timestamp(0),
            },
            FileOperation::Save {
                original_file_name: "file2.txt".to_owned(),
                virtual_path: "my_files/some_file.txt".into(),
                content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                timestamp: get_timestamp(2),
            },
            FileOperation::Save {
                original_file_name: "file3.txt".to_owned(),
                virtual_path: "definitely_not_a_file.txt".into(),
                content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                timestamp: get_timestamp(3),
            },
            FileOperation::Delete {
                virtual_path: "/".into(),
                timestamp: get_timestamp(1),
            },
        ])
        .await;

        assert!(delete_with_invalid_timestamp.is_err());

        match delete_with_invalid_timestamp.err().unwrap() {
            Error::Internal(InternalError::InconsistentTimestamp { existing, inserted }) => {
                assert_eq!(existing, get_timestamp(2));
                assert_eq!(inserted, get_timestamp(1));
            }
            _ => assert!(false),
        };

        Ok(())
    }
}
