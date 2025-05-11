use std::path::Path;

use tokio::fs;

use super::{
    FileDatabase, TimeProvider,
    error::{Error, Result},
    virtual_path::VirtualPath,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileMetadata {
    pub original_file_name: String,
    pub size_in_bytes: usize,
    pub hash: String,
}

impl<T: TimeProvider> FileDatabase<T> {
    /// Push a file to the database and storage.
    ///
    /// This function takes a path to a physical file, a virtual storage path, a
    /// timestamp, and metadata about the file. If the file is not already in the
    /// database, it copies the file to the storage path and updates the database with
    /// the file's metadata. The virtual storage path are then updated to point to the
    /// new file.
    pub async fn save_file(
        &self,
        path_physical_file: impl AsRef<Path>,
        virtual_path: impl Into<VirtualPath>,
        metadata: FileMetadata,
    ) -> Result<()> {
        let virtual_path: VirtualPath = virtual_path.into();

        if !virtual_path.is_file() {
            return Err(Error::NotAVirtualFile(virtual_path));
        }

        let path_copy = self.path_files.join(&metadata.hash);
        let path_storage = virtual_path.path();
        let timestamp_str = self.time_provider.now().to_rfc3339();
        let mut transaction = self.database.begin().await?;

        let current_file_id = sqlx::query!(
            r#"SELECT id as "id!" FROM files WHERE hash = ?;"#,
            metadata.hash
        )
        .fetch_one(&mut *transaction)
        .await;

        let file_id = match current_file_id {
            Ok(record) => record.id,
            Err(error) => match error {
                sqlx::Error::RowNotFound => {
                    if !fs::try_exists(&path_copy).await? {
                        fs::copy(path_physical_file, path_copy).await?;
                    }

                    // Note: sqlite does not support `usize`, so we cast to `i64`.
                    let signed_size = metadata.size_in_bytes as i64;

                    sqlx::query!(
                        r#"
                        INSERT INTO files (original_file_name, size, hash, upload_date)
                        VALUES (?, ?, ?, ?)
                        "#,
                        metadata.original_file_name,
                        signed_size,
                        metadata.hash,
                        timestamp_str
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
            timestamp_str,
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
            timestamp_str
        )
        .execute(&mut *transaction)
        .await?;

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

    /// Test to verify that saving a single file to the storage works as intended.
    ///
    /// The expected behavior is that both the paths and files tables contain exactly
    /// one entry for the new file. The file should also be present in the storage
    /// folder.
    #[tokio::test]
    async fn test_save_single_file() -> Result<()> {
        let original_name_file = "some_file.txt";
        let path = VirtualPath::from("my_files/helloworld.txt");
        let content = b"hello world!".to_vec().into_boxed_slice();

        check_database_state(
            vec![FileOperation::Save {
                original_file_name: original_name_file.to_owned(),
                virtual_path: path.clone(),
                content: content.clone(),
            }],
            FileDatabaseState {
                files: vec![FileDatabaseFile {
                    original_file_name: original_name_file.to_owned(),
                    upload_date: get_timestamp(0),
                    content: content,
                }],
                paths: vec![FileDatabasePath {
                    file_id: 1,
                    path: path,
                    valid_since: get_timestamp(0),
                    valid_until: None,
                }],
            },
        )
        .await
    }

    /// Test to verify that saving several files without any path or content collisions
    /// works as intended.
    ///
    /// The expected behavior is that all saved files are added to the storage folder,
    /// each file has a unique entry in both the paths and files tables, and all paths
    /// are marked as live.
    #[tokio::test]
    async fn test_save_multiple_files() -> Result<()> {
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
                        valid_until: None,
                    },
                    FileDatabasePath {
                        file_id: 2,
                        path: "/my_files/some_file.txt".into(),
                        valid_since: get_timestamp(1),
                        valid_until: None,
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

    /// Test to verify that saving the same file multiple times does not create
    /// redundant copies of the file.
    ///
    /// The expected behavior is that the database first checks if the new file is
    /// already present in its table using the file's SHA-256 hash as a unique
    /// identifier. If the file is already present, an additional path pointing to the
    /// existing file should be created, while the file table remains unchanged.
    #[tokio::test]
    async fn test_save_same_file() -> Result<()> {
        let file_content = b"hello world!".to_vec().into_boxed_slice();

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "some_file.txt".to_owned(),
                    virtual_path: "my_files/helloworld.txt".into(),
                    content: file_content.clone(),
                },
                FileOperation::Save {
                    original_file_name: "some_other_file.txt".to_owned(),
                    virtual_path: "my_files/different/file.txt".into(),
                    content: file_content.clone(),
                },
            ],
            FileDatabaseState {
                files: vec![FileDatabaseFile {
                    original_file_name: "some_file.txt".to_owned(),
                    upload_date: get_timestamp(0),
                    content: file_content.clone(),
                }],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: "/my_files/helloworld.txt".into(),
                        valid_since: get_timestamp(0),
                        valid_until: None,
                    },
                    FileDatabasePath {
                        file_id: 1,
                        path: "/my_files/different/file.txt".into(),
                        valid_since: get_timestamp(1),
                        valid_until: None,
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that overriding a file correctly updates the path table.
    ///
    /// The expected behavior is that the old path is invalidated at the same time the
    /// new path becomes live, effectively replacing the old path with the new one.
    #[tokio::test]
    async fn test_override_existing_file() -> Result<()> {
        let file_base_content = b"hello world!".to_vec().into_boxed_slice();
        let file_over_content = b"evil file override >:)".to_vec().into_boxed_slice();
        let common_path = VirtualPath::from("/my_files/override_me.txt");

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "some_file.txt".to_owned(),
                    virtual_path: common_path.clone(),
                    content: file_base_content.clone(),
                },
                FileOperation::Save {
                    original_file_name: "other_file.txt".to_owned(),
                    virtual_path: common_path.clone(),
                    content: file_over_content.clone(),
                },
            ],
            FileDatabaseState {
                files: vec![
                    FileDatabaseFile {
                        original_file_name: "some_file.txt".to_owned(),
                        upload_date: get_timestamp(0),
                        content: file_base_content.clone(),
                    },
                    FileDatabaseFile {
                        original_file_name: "other_file.txt".to_owned(),
                        upload_date: get_timestamp(1),
                        content: file_over_content.clone(),
                    },
                ],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: common_path.clone(),
                        valid_since: get_timestamp(0),
                        valid_until: Some(get_timestamp(1)),
                    },
                    FileDatabasePath {
                        file_id: 2,
                        path: common_path,
                        valid_since: get_timestamp(1),
                        valid_until: None,
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that uploading the same file to the same path does not modify the
    /// database.
    ///
    /// The expected behavior is that the path table remains unchanged. This avoids
    /// having two paths pointing to the same file, where one is invalidated at the
    /// exact moment the other becomes live.
    #[tokio::test]
    async fn test_override_with_same_file() -> Result<()> {
        let file_content = b"hello world!".to_vec().into_boxed_slice();
        let common_path = VirtualPath::from("/my_files/helloworld.txt");

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "some_file.txt".to_owned(),
                    virtual_path: common_path.clone(),
                    content: file_content.clone(),
                },
                FileOperation::Save {
                    original_file_name: "some_other_file.txt".to_owned(),
                    virtual_path: common_path.clone(),
                    content: file_content.clone(),
                },
            ],
            FileDatabaseState {
                files: vec![FileDatabaseFile {
                    original_file_name: "some_file.txt".to_owned(),
                    upload_date: get_timestamp(0),
                    content: file_content.clone(),
                }],
                paths: vec![FileDatabasePath {
                    file_id: 1,
                    path: common_path,
                    valid_since: get_timestamp(0),
                    valid_until: None,
                }],
            },
        )
        .await
    }

    /// Test to verify that attempting to save a file with an invalid path fails.
    ///
    /// The expected behavior is that an error is returned when the given path points to
    /// the root or a folder instead of a file.
    #[tokio::test]
    async fn test_save_with_invalid_path() -> Result<()> {
        let insert_to_directory_result = setup_test_database(vec![FileOperation::Save {
            original_file_name: "some_file.txt".to_owned(),
            virtual_path: VirtualPath::from("/"),
            content: Box::default(),
        }])
        .await;

        assert!(insert_to_directory_result.is_err());
        assert!(matches!(
            insert_to_directory_result
                .err()
                .expect("The result must be an error."),
            Error::NotAVirtualFile { .. }
        ));

        let insert_to_directory_result = setup_test_database(vec![FileOperation::Save {
            original_file_name: "some_file.txt".to_owned(),
            virtual_path: VirtualPath::from("/path/to/some/dir/"),
            content: Box::default(),
        }])
        .await;

        assert!(insert_to_directory_result.is_err());
        assert!(matches!(
            insert_to_directory_result
                .err()
                .expect("The result must be an error."),
            Error::NotAVirtualFile { .. }
        ));

        Ok(())
    }

    // TODO: remove
    // /// Test to verify that attempting to insert a file with an invalid timestamp fails.
    // ///
    // /// The expected behavior is that an error is returned when trying to override an
    // /// existing file with an older timestamp.
    // #[tokio::test]
    // async fn test_save_with_invalid_timestamp() -> Result<()> {
    //     let common_path = VirtualPath::from("/path/to/file.txt");

    //     let insert_with_invalid_timestamp = setup_test_database(vec![
    //         FileOperation::Save {
    //             original_file_name: "some_file.txt".to_owned(),
    //             virtual_path: common_path.clone(),
    //             content: b"some_file".to_vec().into_boxed_slice(),
    //             timestamp: get_timestamp(1),
    //         },
    //         FileOperation::Save {
    //             original_file_name: "another_file.txt".to_owned(),
    //             virtual_path: common_path.clone(),
    //             content: b"another_file".to_vec().into_boxed_slice(),
    //             timestamp: get_timestamp(0),
    //         },
    //     ])
    //     .await;

    //     assert!(insert_with_invalid_timestamp.is_err());

    //     match insert_with_invalid_timestamp.err().unwrap() {
    //         Error::Internal(InternalError::InconsistentTimestamp { existing, inserted }) => {
    //             assert_eq!(existing, get_timestamp(1));
    //             assert_eq!(inserted, get_timestamp(0));
    //         }
    //         _ => assert!(false),
    //     };

    //     Ok(())
    // }
}
