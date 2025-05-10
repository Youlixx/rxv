use std::io;

use chrono::{DateTime, Utc};

use super::virtual_path::VirtualPath;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InternalError {
    #[error("IO related error: {0}")]
    Io(#[from] io::Error),

    #[error("SQL related error: {0}")]
    Sql(#[from] sqlx::Error),

    #[error("Malformed timestamp within the SQL table: {0}")]
    MalformedTimestamp(#[from] chrono::ParseError),

    #[error("Inconsistent timestamp")]
    InconsistentTimestamp {
        existing: DateTime<Utc>,
        inserted: DateTime<Utc>,
    },
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("Internal error: {0}")]
    Internal(InternalError),

    #[error("the virtual path must point to a file")]
    NotAVirtualFile(VirtualPath),

    #[error("the virtual path must point to a directory")]
    NotAVirtualDirectory(VirtualPath),

    #[error("the virtual file could not be found")]
    VirtualFileNotFound(VirtualPath),

    #[error("Unknown error: {0}")]
    Unknown(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl<T> From<T> for Error
where
    T: Into<InternalError>,
{
    fn from(value: T) -> Self {
        Error::Internal(value.into())
    }
}
