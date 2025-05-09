/// A storage virtual path.
///
/// A simple wrapper around a [`String`], representing a virtual path within the
/// storage. The path is actually stored with a SQL pattern matching symbols.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct VirtualPath {
    pattern: String,
}

impl VirtualPath {
    /// The separator used in the path.
    pub const SEPARATOR: &str = "/";
    const MATCH_PATTERN: &str = "%";

    /// Return whether or not the path points to a file.
    pub fn is_file(&self) -> bool {
        !self.is_dir()
    }

    /// Return whether or not the path points a directory.
    pub fn is_dir(&self) -> bool {
        self.pattern.ends_with(VirtualPath::MATCH_PATTERN)
    }

    /// Get the SQL match pattern associated with the virtual path.
    pub fn match_pattern(&self) -> &str {
        &self.pattern
    }

    /// Get the absolute virtual path.
    pub fn path(&self) -> &str {
        if self.pattern.ends_with(VirtualPath::MATCH_PATTERN) {
            &self.pattern[..self.pattern.len() - VirtualPath::MATCH_PATTERN.len()]
        } else {
            &self.pattern
        }
    }

    /// Get the name of the file or directory pointed by the virtual path.
    ///
    /// Get the name of the object pointed by the virtual path:
    /// - to a file, then the filename is returned.
    /// - to a directory, then the directory name is returned.
    /// - to the root, then [`None`] is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(VirtualPath::from("/some/file").filename(), Some("file"));
    /// assert_eq!(VirtualPath::from("/some/dir/").filename(), Some("dir"));
    /// assert_eq!(VirtualPath::default().filename(), None);
    /// ```
    pub fn filename(&self) -> Option<&str> {
        let path = if self.pattern.ends_with(VirtualPath::MATCH_PATTERN) {
            &self.pattern[..self.pattern.len() - 2]
        } else {
            &self.pattern
        };

        // NOTE: `split` will always return at least one element (the string
        // itself if the split sequence is missing), so we can safely unwrap
        // the option here.
        let filename = path.split(VirtualPath::SEPARATOR).last().unwrap();

        if filename.len() > 0 {
            Some(filename)
        } else {
            None
        }
    }
}

impl<T> From<T> for VirtualPath
where
    T: ToString,
{
    fn from(path: T) -> Self {
        let mut path = path.to_string();

        if !path.starts_with(VirtualPath::SEPARATOR) {
            path = VirtualPath::SEPARATOR.to_owned() + &path;
        }

        if path.ends_with(VirtualPath::SEPARATOR) {
            path += VirtualPath::MATCH_PATTERN;
        }

        Self { pattern: path }
    }
}

impl Default for VirtualPath {
    fn default() -> Self {
        VirtualPath::from("")
    }
}

#[cfg(test)]
mod tests {
    use super::VirtualPath;

    #[test]
    fn test_pattern_formatting() {
        let path_file = VirtualPath::from("/path/to/some/file");
        assert_eq!(path_file.path(), "/path/to/some/file");
        assert_eq!(path_file.match_pattern(), "/path/to/some/file");

        let path_dir = VirtualPath::from("/path/to/some/dir/");
        assert_eq!(path_dir.path(), "/path/to/some/dir/");
        assert_eq!(path_dir.match_pattern(), "/path/to/some/dir/%");

        let path_root = VirtualPath::from("/");
        assert_eq!(path_root.path(), "/");
        assert_eq!(path_root.match_pattern(), "/%");

        let path_file = VirtualPath::from("path/to/some/file");
        assert_eq!(path_file.path(), "/path/to/some/file");
        assert_eq!(path_file.match_pattern(), "/path/to/some/file");

        let path_dir = VirtualPath::from("path/to/some/dir/");
        assert_eq!(path_dir.path(), "/path/to/some/dir/");
        assert_eq!(path_dir.match_pattern(), "/path/to/some/dir/%");

        let path_root = VirtualPath::from("");
        assert_eq!(path_root.path(), "/");
        assert_eq!(path_root.match_pattern(), "/%");
    }

    #[test]
    fn test_is_dir_and_file() {
        let path_file = VirtualPath::from("/path/to/some/file");
        assert!(path_file.is_file());
        assert!(!path_file.is_dir());

        let path_dir = VirtualPath::from("/path/to/some/dir/");
        assert!(!path_dir.is_file());
        assert!(path_dir.is_dir());

        let path_root = VirtualPath::from("/");
        assert!(!path_root.is_file());
        assert!(path_root.is_dir());
    }

    #[test]
    fn test_filename() {
        let path_file = VirtualPath::from("/path/to/some/file");
        assert_eq!(path_file.filename(), Some("file"));

        let path_dir = VirtualPath::from("/path/to/some/dir/");
        assert_eq!(path_dir.filename(), Some("dir"));

        let path_root = VirtualPath::from("/");
        assert_eq!(path_root.filename(), None);
    }
}
