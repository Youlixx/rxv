/// A specialized `Result` type for rvx.
pub type Result<T, E = Error> = ::std::result::Result<T, E>;


#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("sql related error")]
    Sql(#[from] sqlx::Error)
}
