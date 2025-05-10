use chrono::{DateTime, Utc};

use super::{
    FileDatabase,
    error::{Error, Result},
    save_file::FileMetadata,
    virtual_path::VirtualPath,
};

pub struct PathMetadataPair {
    pub virtual_path: VirtualPath,
    pub metadata: FileMetadata,
    pub upload_timestamp: DateTime<Utc>,
}

impl FileDatabase {
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

        let query = sqlx::query!(
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
        );

        Ok(query
            .fetch_all(&self.database)
            .await?
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
