use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::{FileDatabase, TimeProvider, error::Result, virtual_path::VirtualPath};

#[derive(Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path_physical_file: PathBuf,
    pub virtual_path: VirtualPath,
}

#[derive(Debug, Eq, PartialEq)]
pub enum FileEntries {
    None,
    SingleFile(FileEntry),
    MultipleFiles(Vec<FileEntry>),
}

impl<T: TimeProvider> FileDatabase<T> {
    async fn get_single_file_path(
        &self,
        virtual_path: VirtualPath,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<FileEntry>> {
        let timestamp_str = timestamp.to_rfc3339();
        let path_storage = virtual_path.path();

        let query = sqlx::query!(
            r#"
            SELECT files.hash FROM files
            INNER JOIN paths ON files.id == paths.file_id
            WHERE ? >= paths.valid_since
                AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                AND paths.path == ?;
            "#,
            timestamp_str,
            timestamp_str,
            path_storage
        );

        Ok(query
            .fetch_optional(&self.database)
            .await?
            .map(|file| FileEntry {
                path_physical_file: self.get_physical_file_path(&file.hash),
                virtual_path,
            }))
    }

    async fn get_multiple_file_paths(
        &self,
        virtual_path: VirtualPath,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<FileEntry>> {
        let timestamp_str = timestamp.to_rfc3339();
        let path_wildcard = virtual_path.match_pattern();

        let query = sqlx::query!(
            r#"
            SELECT files.hash, paths.path FROM files
            INNER JOIN paths ON files.id == paths.file_id
            WHERE ? >= paths.valid_since
                AND ? < COALESCE(paths.valid_until, '9999-12-31T23:59:59Z')
                AND paths.path LIKE ?;
            "#,
            timestamp_str,
            timestamp_str,
            path_wildcard
        );

        Ok(query
            .fetch_all(&self.database)
            .await?
            .into_iter()
            .map(|file| FileEntry {
                path_physical_file: self.get_physical_file_path(&file.hash),
                virtual_path: VirtualPath::from(file.path),
            })
            .collect::<Vec<_>>())
    }

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
    pub async fn get_file(
        &self,
        virtual_path: impl Into<VirtualPath>,
        timestamp: DateTime<Utc>,
    ) -> Result<FileEntries> {
        let virtual_path: VirtualPath = virtual_path.into();

        let files = if virtual_path.is_dir() {
            let entries = self
                .get_multiple_file_paths(virtual_path, timestamp)
                .await?;

            if entries.is_empty() {
                FileEntries::None
            } else {
                FileEntries::MultipleFiles(entries)
            }
        } else {
            match self.get_single_file_path(virtual_path, timestamp).await? {
                Some(file_entry) => FileEntries::SingleFile(file_entry),
                None => FileEntries::None,
            }
        };

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{
        error::Result,
        get_file::{FileEntries, FileEntry},
        tests::{FileOperation, get_hash, get_timestamp, setup_test_database},
        virtual_path::VirtualPath,
    };

    /// Test to verify that getting a single file returns the expected path.
    ///
    /// The expected behavior is that the function returns a
    /// [`FileEntries::File`] with a path to the actual file on the server
    /// disk.
    #[tokio::test]
    async fn test_get_single_file() -> Result<()> {
        let path = VirtualPath::from("/my_files/helloworld.txt");
        let content = b"hello world!".to_vec().into_boxed_slice();

        let (_test_dir, database) = setup_test_database(vec![FileOperation::Save {
            original_file_name: "some_file.txt".to_owned(),
            virtual_path: path.clone(),
            content: content.clone(),
        }])
        .await?;

        assert_eq!(
            database.get_file(path.clone(), get_timestamp(1)).await?,
            FileEntries::SingleFile(FileEntry {
                path_physical_file: database.get_physical_file_path(&get_hash(&content)),
                virtual_path: path
            })
        );

        Ok(())
    }

    /// Test to verify that getting a directory returns the expected paths.
    ///
    /// The expected behavior is that the function returns a
    /// [`FileEntries::Directory`] where each entry is a tuple of the path to
    /// the physical disk on the server side and the corresponding storage path.
    #[tokio::test]
    async fn test_get_directory_files() -> Result<()> {
        let files = [
            (
                VirtualPath::from("/my_files/helloworld.txt"),
                b"hello world!".to_vec().into_boxed_slice(),
            ),
            (
                VirtualPath::from("/my_files/some_file.txt"),
                b"I'm a sample file!".to_vec().into_boxed_slice(),
            ),
        ];

        let (_test_dir, database) = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "file1.txt".to_owned(),
                virtual_path: files[0].0.clone(),
                content: files[0].1.clone(),
            },
            FileOperation::Save {
                original_file_name: "file2.txt".to_owned(),
                virtual_path: files[1].0.clone(),
                content: files[1].1.clone(),
            },
            FileOperation::Save {
                original_file_name: "file3.txt".to_owned(),
                virtual_path: "definitely_not_a_file.txt".into(),
                content: b"I'm not a file :)".to_vec().into_boxed_slice(),
            },
        ])
        .await?;

        assert_eq!(
            database
                .get_file(VirtualPath::from("/my_files/"), get_timestamp(4))
                .await?,
            FileEntries::MultipleFiles(vec![
                FileEntry {
                    path_physical_file: database.get_physical_file_path(&get_hash(&files[0].1)),
                    virtual_path: files[0].0.clone()
                },
                FileEntry {
                    path_physical_file: database.get_physical_file_path(&get_hash(&files[1].1)),
                    virtual_path: files[1].0.clone()
                },
            ])
        );

        Ok(())
    }

    /// Test to verify that getting a directory with a single file returns the
    /// expected paths.
    ///
    /// The expected behavior is that, even though the folder contains only a
    /// single file, the function should still return
    /// [`FileEntries::Directory`] since we are querying for a directory. The
    /// path list should contain a single entry mapping to the file.
    #[tokio::test]
    async fn test_get_directory_single_file() -> Result<()> {
        let path = VirtualPath::from("/my_files/helloworld.txt");
        let content = b"hello world!".to_vec().into_boxed_slice();

        let (_test_dir, database) = setup_test_database(vec![FileOperation::Save {
            original_file_name: "some_file.txt".to_owned(),
            virtual_path: path.clone(),
            content: content.clone(),
        }])
        .await?;

        assert_eq!(
            database
                .get_file(VirtualPath::from("/my_files/"), get_timestamp(1))
                .await?,
            FileEntries::MultipleFiles(vec![FileEntry {
                path_physical_file: database.get_physical_file_path(&get_hash(&content)),
                virtual_path: path
            }])
        );

        Ok(())
    }

    /// Test to verify that requesting the root returns all currently live paths
    /// from the storage.
    ///
    /// The expected behavior is that the function returns a
    /// [`FileEntries::Directory`] containing all storage files.
    #[tokio::test]
    async fn test_get_all_files() -> Result<()> {
        let files = [
            (
                VirtualPath::from("/my_files/helloworld.txt"),
                b"hello world!".to_vec().into_boxed_slice(),
            ),
            (
                VirtualPath::from("/my_files/some_file.txt"),
                b"I'm a sample file!".to_vec().into_boxed_slice(),
            ),
            (
                VirtualPath::from("/definitely_not_a_file.txt"),
                b"I'm not a file :)".to_vec().into_boxed_slice(),
            ),
        ];

        let (_test_dir, database) = setup_test_database(
            files
                .iter()
                .enumerate()
                .map(|(index, file)| FileOperation::Save {
                    original_file_name: index.to_string() + ".txt",
                    virtual_path: file.0.clone(),
                    content: file.1.clone(),
                })
                .collect::<Vec<_>>(),
        )
        .await?;

        assert_eq!(
            database
                .get_file(VirtualPath::from("/"), get_timestamp(4))
                .await?,
            FileEntries::MultipleFiles(
                files
                    .into_iter()
                    .map(|file| FileEntry {
                        path_physical_file: database.get_physical_file_path(&get_hash(&file.1)),
                        virtual_path: file.0
                    })
                    .collect::<Vec<_>>()
            )
        );

        Ok(())
    }

    /// Test to verify that requesting a non-existent file returns nothing.
    ///
    /// The expected behavior is that, whether requesting a file or a directory,
    /// the function should return [`FileEntries::None`] in both cases.
    #[tokio::test]
    async fn test_get_invalid_file() -> Result<()> {
        let (_test_dir, database) = setup_test_database(vec![
            FileOperation::Save {
                original_file_name: "some_file.txt".to_owned(),
                virtual_path: VirtualPath::from("/my_files/helloworld.txt"),
                content: b"hello world!".to_vec().into_boxed_slice(),
            },
            FileOperation::Save {
                original_file_name: "another_file.txt".to_owned(),
                virtual_path: VirtualPath::from("/my_new_file.txt"),
                content: b"hello world!".to_vec().into_boxed_slice(),
            },
        ])
        .await?;

        assert_eq!(
            database
                .get_file(VirtualPath::from("/my_files/unknown.txt"), get_timestamp(2))
                .await?,
            FileEntries::None
        );

        assert_eq!(
            database
                .get_file(VirtualPath::from("/unknown_dir/"), get_timestamp(2))
                .await?,
            FileEntries::None
        );

        assert_eq!(
            database
                .get_file(
                    VirtualPath::from("/my_new_file.txt"),
                    get_timestamp(0)
                )
                .await?,
            FileEntries::None
        );

        Ok(())
    }
}
