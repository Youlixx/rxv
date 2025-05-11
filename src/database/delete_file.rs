use super::{error::{Error, Result}, virtual_path::VirtualPath, FileDatabase, TimeProvider};

impl<T: TimeProvider> FileDatabase<T> {
    /// Delete a file from the current storage.
    ///
    /// This function does not delete the actual files on the disk. Instead, it
    /// marks the specified storage paths as invalid, effectively removing them
    /// from the live storage.
    pub async fn delete_file(&self, virtual_path: impl Into<VirtualPath>) -> Result<()> {
        let virtual_path: VirtualPath = virtual_path.into();
        let timestamp = self.time_provider.now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        let affected_rows = if virtual_path.is_file() {
            let path_storage = virtual_path.path();

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
            .await?
        } else {
            let matching_pattern = virtual_path.match_pattern();

            sqlx::query!(
                r#"
                UPDATE paths
                SET valid_until = ?
                WHERE path LIKE ? AND valid_until IS NULL;
                "#,
                timestamp,
                matching_pattern
            )
            .execute(&mut *transaction)
            .await?
        };

        if affected_rows.rows_affected() == 0 {
            return Err(Error::VirtualFileNotFound(virtual_path));
        }

        transaction.commit().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{
        error::{Error, Result},
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
                },
                FileOperation::Delete {
                    virtual_path: path.clone(),
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
                },
                FileOperation::Save {
                    original_file_name: "file2.txt".to_owned(),
                    virtual_path: "my_files/some_file.txt".into(),
                    content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                },
                FileOperation::Save {
                    original_file_name: "file3.txt".to_owned(),
                    virtual_path: "definitely_not_a_file.txt".into(),
                    content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                },
                FileOperation::Delete {
                    virtual_path: "my_files/".into(),
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
                },
                FileOperation::Save {
                    original_file_name: "file2.txt".to_owned(),
                    virtual_path: "my_files/some_file.txt".into(),
                    content: b"I'm a sample file!".to_vec().into_boxed_slice(),
                },
                FileOperation::Save {
                    original_file_name: "file3.txt".to_owned(),
                    virtual_path: "definitely_not_a_file.txt".into(),
                    content: b"I'm not a file :)".to_vec().into_boxed_slice(),
                },
                FileOperation::Delete {
                    virtual_path: "/".into(),
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
}
