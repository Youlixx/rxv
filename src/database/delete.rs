use chrono::Utc;

use crate::{
    path::StoragePath,
    response::{Error, Result},
};

use super::AppState;

impl AppState {
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
mod tests {}
