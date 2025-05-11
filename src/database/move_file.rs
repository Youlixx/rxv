use chrono::{DateTime, Utc};
use sqlx::{Sqlite, Transaction};

use super::{
    FileDatabase, TimeProvider,
    error::{Error, Result},
    virtual_path::VirtualPath,
};

async fn update_file_path(
    file_id: i64,
    path_new: &str,
    timestamp_str: &str,
    transaction: &mut Transaction<'static, Sqlite>,
) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE paths
        SET valid_until = ?
        WHERE path = ? AND valid_until IS NULL;
        "#,
        timestamp_str,
        path_new
    )
    .execute(&mut **transaction)
    .await?;

    sqlx::query!(
        r#"
        INSERT INTO paths (file_id, path, valid_since, valid_until)
        VALUES (?, ?, ?, NULL);
        "#,
        file_id,
        path_new,
        timestamp_str
    )
    .execute(&mut **transaction)
    .await?;

    Ok(())
}

impl<T: TimeProvider> FileDatabase<T> {
    async fn move_single_file(
        &self,
        path_old: VirtualPath,
        path_new: VirtualPath,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        if !path_old.is_file() {
            return Err(Error::NotAVirtualFile(path_old));
        }

        if !path_new.is_file() {
            return Err(Error::NotAVirtualFile(path_new));
        }

        let path_old_str = path_old.path();
        let mut transaction: Transaction<'static, Sqlite> = self.database.begin().await?;

        let file = sqlx::query!(
            r#"
            SELECT id, file_id, valid_since FROM paths
            WHERE path = ? AND valid_until IS NULL;
            "#,
            path_old_str
        )
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(Error::VirtualFileNotFound(path_old))?;

        let timestamp_str = timestamp.to_rfc3339();

        sqlx::query!(
            r#"UPDATE paths SET valid_until = ? WHERE id = ?"#,
            timestamp_str,
            file.id
        )
        .execute(&mut *transaction)
        .await?;

        update_file_path(
            file.file_id,
            path_new.path(),
            &timestamp.to_rfc3339(),
            &mut transaction,
        )
        .await?;

        transaction.commit().await?;

        Ok(())
    }

    async fn move_directory(
        &self,
        path_old: VirtualPath,
        path_new: VirtualPath,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        if !path_old.is_dir() {
            return Err(Error::NotAVirtualDirectory(path_old));
        }

        if !path_new.is_dir() {
            return Err(Error::NotAVirtualDirectory(path_new));
        }

        let matching_pattern = path_old.match_pattern();
        let mut transaction = self.database.begin().await?;

        let files = sqlx::query!(
            r#"
            SELECT id, file_id, path, valid_since FROM paths
            WHERE path LIKE ? AND valid_until IS NULL;
            "#,
            matching_pattern
        )
        .fetch_all(&mut *transaction)
        .await?;

        if files.is_empty() {
            return Err(Error::VirtualFileNotFound(path_old));
        }

        let timestamp_str = timestamp.to_rfc3339();

        sqlx::query!(
            r#"
            UPDATE paths
            SET valid_until = ?
            WHERE path LIKE ? AND valid_until IS NULL
            "#,
            timestamp_str,
            matching_pattern
        )
        .execute(&mut *transaction)
        .await?;

        for file in &files {
            let path_new_str = path_new.path().to_string() + &file.path[path_old.path().len()..];

            update_file_path(
                file.file_id,
                &path_new_str,
                &timestamp_str,
                &mut transaction,
            )
            .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    pub async fn move_file(
        &self,
        path_old: impl Into<VirtualPath>,
        path_new: impl Into<VirtualPath>,
    ) -> Result<()> {
        let path_old: VirtualPath = path_old.into();
        let path_new: VirtualPath = path_new.into();
        let timestamp = self.time_provider.now();

        if path_old == path_new {
            Ok(())
        } else if path_old.is_file() && path_new.is_file() {
            self.move_single_file(path_old, path_new, timestamp).await
        } else if path_old.is_dir() && path_new.is_dir() {
            self.move_directory(path_old, path_new, timestamp).await
        } else {
            Err(Error::InconsistentVirtualPaths { path_old, path_new })
        }
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

    /// Test to verify that moving a single file, without any path conflict works as
    /// expected.
    #[tokio::test]
    async fn test_move_single_file() -> Result<()> {
        let path_old = VirtualPath::from("/some/file.txt");
        let path_new = VirtualPath::from("/another/file_v2.txt");
        let content = b"file_content".to_vec().into_boxed_slice();

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "file.txt".to_owned(),
                    virtual_path: path_old.clone(),
                    content: content.clone(),
                },
                FileOperation::Move {
                    path_old: path_old.clone(),
                    path_new: path_new.clone(),
                },
            ],
            FileDatabaseState {
                files: vec![FileDatabaseFile {
                    original_file_name: "file.txt".to_owned(),
                    upload_date: get_timestamp(0),
                    content: content,
                }],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: path_old,
                        valid_since: get_timestamp(0),
                        valid_until: Some(get_timestamp(1)),
                    },
                    FileDatabasePath {
                        file_id: 1,
                        path: path_new,
                        valid_since: get_timestamp(1),
                        valid_until: None,
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that moving a single file, with a path conflict works as
    /// expected.
    #[tokio::test]
    async fn test_move_single_file_override() -> Result<()> {
        let path_old = VirtualPath::from("/some/file.txt");
        let path_new = VirtualPath::from("/another/file_v2.txt");
        let content_moved = b"file_content".to_vec().into_boxed_slice();
        let content_overwritten = b"hello world!".to_vec().into_boxed_slice();

        check_database_state(
            vec![
                FileOperation::Save {
                    original_file_name: "file.txt".to_owned(),
                    virtual_path: path_old.clone(),
                    content: content_moved.clone(),
                },
                FileOperation::Save {
                    original_file_name: "another.txt".to_owned(),
                    virtual_path: path_new.clone(),
                    content: content_overwritten.clone(),
                },
                FileOperation::Move {
                    path_old: path_old.clone(),
                    path_new: path_new.clone(),
                },
            ],
            FileDatabaseState {
                files: vec![
                    FileDatabaseFile {
                        original_file_name: "file.txt".to_owned(),
                        upload_date: get_timestamp(0),
                        content: content_moved,
                    },
                    FileDatabaseFile {
                        original_file_name: "another.txt".to_owned(),
                        upload_date: get_timestamp(1),
                        content: content_overwritten,
                    },
                ],
                paths: vec![
                    FileDatabasePath {
                        file_id: 1,
                        path: path_old,
                        valid_since: get_timestamp(0),
                        valid_until: Some(get_timestamp(2)),
                    },
                    FileDatabasePath {
                        file_id: 2,
                        path: path_new.clone(),
                        valid_since: get_timestamp(1),
                        valid_until: Some(get_timestamp(2)),
                    },
                    FileDatabasePath {
                        file_id: 1,
                        path: path_new,
                        valid_since: get_timestamp(2),
                        valid_until: None,
                    },
                ],
            },
        )
        .await
    }

    /// Test to verify that moving a non-existing file fails.
    #[tokio::test]
    async fn test_move_missing_file() -> Result<()> {
        let path_old = VirtualPath::from("/some/file.txt");
        let path_new = VirtualPath::from("/another/file_v2.txt");

        let move_result = setup_test_database(vec![FileOperation::Move {
            path_old: path_old.clone(),
            path_new: path_new,
        }])
        .await;

        assert!(move_result.is_err());

        match move_result.err().expect("The result must be an error.") {
            Error::VirtualFileNotFound(path) => assert_eq!(path, path_old),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that moving a non-live file fails.
    #[tokio::test]
    async fn test_move_non_live_file() -> Result<()> {
        let path_old = VirtualPath::from("/some/file.txt");
        let path_new = VirtualPath::from("/another/file_v2.txt");

        let move_result = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "some_file.txt".to_owned(),
                virtual_path: path_old.clone(),
                content: b"hey".to_vec().into_boxed_slice(),
            },
            FileOperation::Delete {
                virtual_path: path_old.clone(),
            },
            FileOperation::Move {
                path_old: path_old.clone(),
                path_new: path_new,
            },
        ])
        .await;

        assert!(move_result.is_err());

        match move_result.err().expect("The result must be an error.") {
            Error::VirtualFileNotFound(path) => assert_eq!(path, path_old),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that moving a directory without any path conflict works as
    /// expected.
    #[tokio::test]
    async fn test_move_directory() -> Result<()> {
        let files: [(_, _, &[u8]); 4] = [
            ("file1.txt", "my_files/helloworld.txt", b"hello world!"),
            ("file2.txt", "my_files/some_file.txt", b"I'm a sample file!"),
            (
                "file3.txt",
                "definitely_not_a_file.txt",
                b"I'm not a file :)",
            ),
            ("file4.txt", "dest_dir/hey.txt", b"hey"),
        ];

        let mut operations = files
            .iter()
            .map(|(filename, virtual_path, content)| FileOperation::Save {
                original_file_name: filename.to_string(),
                virtual_path: VirtualPath::from(virtual_path),
                content: content.to_vec().into_boxed_slice(),
            })
            .collect::<Vec<_>>();

        operations.push(FileOperation::Move {
            path_old: VirtualPath::from("my_files/"),
            path_new: VirtualPath::from("dest_dir/"),
        });

        let move_timestamp = get_timestamp(files.len());

        let mut paths = files
            .iter()
            .enumerate()
            .map(|(index, (_, virtual_path, _))| FileDatabasePath {
                file_id: index + 1,
                path: VirtualPath::from(virtual_path),
                valid_since: get_timestamp(index),
                valid_until: virtual_path
                    .contains("my_files/")
                    .then_some(move_timestamp.clone()),
            })
            .collect::<Vec<_>>();

        for (index, (_, virtual_path, _)) in files.iter().enumerate() {
            if !virtual_path.contains("my_files") {
                continue;
            }

            paths.push(FileDatabasePath {
                file_id: index + 1,
                path: VirtualPath::from(virtual_path.replace("my_files", "dest_dir")),
                valid_since: move_timestamp.clone(),
                valid_until: None,
            });
        }

        check_database_state(
            operations,
            FileDatabaseState {
                files: files
                    .iter()
                    .enumerate()
                    .map(|(index, (filename, _, content))| FileDatabaseFile {
                        original_file_name: filename.to_string(),
                        upload_date: get_timestamp(index),
                        content: content.to_vec().into_boxed_slice(),
                    })
                    .collect(),
                paths,
            },
        )
        .await
    }

    /// Test to verify that moving a directory with a path conflict works as expected.
    #[tokio::test]
    async fn test_move_directory_override() -> Result<()> {
        let files: [(_, _, &[u8]); 4] = [
            ("file1.txt", "my_files/hey.txt", b"hello world!"),
            ("file2.txt", "my_files/some_file.txt", b"I'm a sample file!"),
            (
                "file3.txt",
                "definitely_not_a_file.txt",
                b"I'm not a file :)",
            ),
            ("file4.txt", "dest_dir/hey.txt", b"hey"),
        ];

        let mut operations = files
            .iter()
            .map(|(filename, virtual_path, content)| FileOperation::Save {
                original_file_name: filename.to_string(),
                virtual_path: VirtualPath::from(virtual_path),
                content: content.to_vec().into_boxed_slice(),
            })
            .collect::<Vec<_>>();

        operations.push(FileOperation::Move {
            path_old: VirtualPath::from("my_files/"),
            path_new: VirtualPath::from("dest_dir/"),
        });

        let move_timestamp = get_timestamp(files.len());

        let mut paths = files
            .iter()
            .enumerate()
            .map(|(index, (_, virtual_path, _))| FileDatabasePath {
                file_id: index + 1,
                path: VirtualPath::from(virtual_path),
                valid_since: get_timestamp(index),
                valid_until: (virtual_path.contains("my_files/")
                    || virtual_path.contains("dest_dir/"))
                .then_some(move_timestamp.clone()),
            })
            .collect::<Vec<_>>();

        for (index, (_, virtual_path, _)) in files.iter().enumerate() {
            if !virtual_path.contains("my_files") {
                continue;
            }

            paths.push(FileDatabasePath {
                file_id: index + 1,
                path: VirtualPath::from(virtual_path.replace("my_files", "dest_dir")),
                valid_since: move_timestamp.clone(),
                valid_until: None,
            });
        }

        check_database_state(
            operations,
            FileDatabaseState {
                files: files
                    .iter()
                    .enumerate()
                    .map(|(index, (filename, _, content))| FileDatabaseFile {
                        original_file_name: filename.to_string(),
                        upload_date: get_timestamp(index),
                        content: content.to_vec().into_boxed_slice(),
                    })
                    .collect(),
                paths,
            },
        )
        .await
    }

    /// Test to verify that moving a non-existing directory fails.
    #[tokio::test]
    async fn test_move_missing_directory() -> Result<()> {
        let path_old = VirtualPath::from("/some/");
        let path_new = VirtualPath::from("/another/");

        let move_result = setup_test_database(vec![FileOperation::Move {
            path_old: path_old.clone(),
            path_new: path_new,
        }])
        .await;

        assert!(move_result.is_err());

        match move_result.err().expect("The result must be an error.") {
            Error::VirtualFileNotFound(path) => assert_eq!(path, path_old),
            _ => assert!(false),
        }

        Ok(())
    }

    /// Test to verify that trying to move a file into a directory or a directory into a
    /// file fails.
    #[tokio::test]
    async fn test_move_inconsistent_paths() -> Result<()> {
        let path_dir = VirtualPath::from("/some/");
        let path_file = VirtualPath::from("/another/file.txt");

        let move_result = setup_test_database(vec![FileOperation::Move {
            path_old: path_dir.clone(),
            path_new: path_file.clone(),
        }])
        .await;

        assert!(move_result.is_err());

        match move_result.err().expect("The result must be an error.") {
            Error::InconsistentVirtualPaths { path_old, path_new } => {
                assert_eq!(path_old, path_dir.clone());
                assert_eq!(path_new, path_file.clone());
            }
            _ => assert!(false),
        }

        let move_result = setup_test_database(vec![FileOperation::Move {
            path_old: path_file.clone(),
            path_new: path_dir.clone(),
        }])
        .await;

        assert!(move_result.is_err());

        match move_result.err().expect("The result must be an error.") {
            Error::InconsistentVirtualPaths { path_old, path_new } => {
                assert_eq!(path_new, path_dir);
                assert_eq!(path_old, path_file);
            }
            _ => assert!(false),
        }

        Ok(())
    }
}
