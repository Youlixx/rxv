/// A storage relative path.
///
/// A simple wrapper around a [`String`], representing a path relative to the
/// storage. It allows to perform simple path manipulation within the storage
/// space.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct StoragePath(String);

impl StoragePath {
    const SEPARATOR: &str = "/";
    const ROOT_FILENAME: &str = "storage";

    /// Return whether or not the path points to a file.
    pub fn is_file(&self) -> bool {
        !self.is_root() && !self.is_dir()
    }

    /// Return whether or not the path points to the root.
    pub fn is_root(&self) -> bool {
        self.0.len() == 0
    }

    /// Return whether or not the path points to a directory.
    pub fn is_dir(&self) -> bool {
        self.0.ends_with(StoragePath::SEPARATOR) || self.is_root()
    }

    /// Return a string pointer to the wrapped String.
    pub fn to_str(&self) -> &str {
        &self.0
    }

    /// Get the matching pattern used in SQL request.
    ///
    /// For files, this function return the exact path, and for folder or root,
    /// it inserts a SQL wildcard.
    pub fn get_sql_matching_pattern(&self) -> String {
        if self.is_file() {
            self.0.clone()
        } else {
            format!("{}%", self.0)
        }
    }

    /// Get the filename of the object pointed by the path.
    ///
    /// This function cover the three following cases :
    /// - if the path points to the root, a default file name is returned.
    /// - if the path points to a directory, the directory name is returned.
    /// - if the path points to a file, the filename, including its extension
    ///   is returned.
    pub fn filename(&self) -> &str {
        if self.is_root() {
            return StoragePath::ROOT_FILENAME;
        }

        let path_without_trailing_slash = if !self.is_file() {
            &self.0[..self.0.len() - 1]
        } else {
            &self.0
        };

        // NOTE: `split` will always return at least one element (the string
        // itself if the split sequence is missing), so we can safely unwrap
        // the option here.
        path_without_trailing_slash
            .split(StoragePath::SEPARATOR)
            .last()
            .unwrap()
    }

    /// Using the current path as prefix, removes it from a given subpath.
    ///
    /// This function should only be used if the prefix is pointing to a file,
    /// and is mainly used to generate archive paths.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(
    ///     StoragePath::from("path/to/dir/")
    ///         .remove_prefix(&"path/to/dir/subpath/myfile".into()),
    ///     Some("dir/subpath/myfile".into())
    /// )
    /// ```
    pub fn remove_prefix(&self, subpath: &StoragePath) -> Option<StoragePath> {
        if self.is_file() {
            None
        } else if self.is_root() {
            Some(subpath.clone())
        } else if !subpath.0.starts_with(&self.0) {
            None
        } else {
            let prefix_length = self.0[..self.0.len() - 1]
                .rfind(StoragePath::SEPARATOR)
                .map(|position| position + 1)
                .unwrap_or(0);

            Some(StoragePath(subpath.0[prefix_length..].into()))
        }
    }
}

impl<T> From<T> for StoragePath
where
    T: ToString,
{
    fn from(value: T) -> Self {
        let path = value.to_string();

        let path = if path.starts_with(StoragePath::SEPARATOR) {
            &path[1..]
        } else {
            &path
        };

        Self(path.into())
    }
}

#[cfg(test)]
mod tests {
    use super::StoragePath;

    #[test]
    fn test_check_file() {
        let path_file = StoragePath::from("path/to/file");
        assert!(path_file.is_file());
        assert!(!path_file.is_dir());
        assert!(!path_file.is_root());

        let path_dir = StoragePath::from("path/to/dir/");
        assert!(!path_dir.is_file());
        assert!(path_dir.is_dir());
        assert!(!path_dir.is_root());

        let path_root = StoragePath::from("");
        assert!(!path_root.is_file());
        assert!(path_root.is_dir());
        assert!(path_root.is_root());
    }

    #[test]
    fn test_filename() {
        assert_eq!(StoragePath::from("path/to/file").filename(), "file");
        assert_eq!(StoragePath::from("path/to/dir/").filename(), "dir");
        assert_eq!(StoragePath::from("").filename(), StoragePath::ROOT_FILENAME);
    }

    #[test]
    fn test_remove_prefix() {
        assert_eq!(
            StoragePath::from("path/to/dir/").remove_prefix(&"path/to/dir/subpath/myfile".into()),
            Some("dir/subpath/myfile".into())
        );

        assert_eq!(
            StoragePath::from("").remove_prefix(&"path/to/dir/subpath/myfile".into()),
            Some("path/to/dir/subpath/myfile".into())
        );

        assert_eq!(
            StoragePath::from("path/to/dir/")
                .remove_prefix(&"path/to/another/dir/subpath/myfile".into()),
            None
        );

        assert_eq!(
            StoragePath::from("path/to/dir").remove_prefix(&"path/to/dir/subpath/myfile".into()),
            None
        );
    }
}
