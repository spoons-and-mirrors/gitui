use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::ffi::OsString;

use anyhow::Result;

#[cfg(not(unix))]
use anyhow::Context;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RepoPath(PathBuf);

impl RepoPath {
    pub(crate) fn from_git_bytes(bytes: &[u8]) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(Self(PathBuf::from(OsString::from_vec(bytes.to_vec()))))
        }
        #[cfg(windows)]
        {
            let path = String::from_utf8(bytes.to_vec())
                .context("Git returned a repository path that is not valid UTF-8 on Windows")?;
            Ok(Self(PathBuf::from(path)))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let path = String::from_utf8(bytes.to_vec())
                .context("Git returned a repository path that is not valid UTF-8")?;
            Ok(Self(PathBuf::from(path)))
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn as_os_str(&self) -> &OsStr {
        self.0.as_os_str()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.as_os_str().is_empty()
    }

    pub(crate) fn is_utf8(&self) -> bool {
        self.0.to_str().is_some()
    }

    pub(crate) fn display(&self) -> String {
        display_os_str(self.0.as_os_str())
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        self.0
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Self::from)
    }

    pub(crate) fn file_name(&self) -> Option<&OsStr> {
        self.0.file_name()
    }

    pub(crate) fn join(&self, name: &OsStr) -> Self {
        Self(self.0.join(name))
    }

    pub(crate) fn byte_len(&self) -> usize {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            self.0.as_os_str().as_bytes().len()
        }
        #[cfg(not(unix))]
        {
            self.display().len()
        }
    }

    pub(crate) fn git_bytes(&self) -> Result<Vec<u8>> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Ok(self.0.as_os_str().as_bytes().to_vec())
        }
        #[cfg(not(unix))]
        {
            Ok(self
                .0
                .to_str()
                .context("Repository path is not valid UTF-8 on this platform")?
                .as_bytes()
                .to_vec())
        }
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display())
    }
}

impl AsRef<Path> for RepoPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<PathBuf> for RepoPath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<&Path> for RepoPath {
    fn from(path: &Path) -> Self {
        Self(path.to_owned())
    }
}

impl From<&str> for RepoPath {
    fn from(path: &str) -> Self {
        Self(PathBuf::from(path))
    }
}

impl From<String> for RepoPath {
    fn from(path: String) -> Self {
        Self(PathBuf::from(path))
    }
}

impl PartialEq<str> for RepoPath {
    fn eq(&self, other: &str) -> bool {
        self.0 == Path::new(other)
    }
}

impl PartialEq<&str> for RepoPath {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

pub(crate) fn display_os_str(value: &OsStr) -> String {
    if let Some(value) = value.to_str() {
        return value.to_owned();
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = value.as_bytes();
        let mut rendered = String::new();
        let mut offset = 0;
        while offset < bytes.len() {
            match std::str::from_utf8(&bytes[offset..]) {
                Ok(valid) => {
                    rendered.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    rendered.push_str(
                        std::str::from_utf8(&bytes[offset..valid_end]).expect("valid UTF-8 prefix"),
                    );
                    let invalid_len = error.error_len().unwrap_or(bytes.len() - valid_end);
                    for byte in &bytes[valid_end..valid_end + invalid_len] {
                        use std::fmt::Write;
                        write!(rendered, "\\x{byte:02X}").expect("writing to String cannot fail");
                    }
                    offset = valid_end + invalid_len;
                }
            }
        }
        rendered
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_utf8_paths_unchanged() {
        assert_eq!(RepoPath::from("src/café.rs").display(), "src/café.rs");
    }

    #[cfg(unix)]
    #[test]
    fn renders_distinct_invalid_bytes_as_escapes() {
        let first = RepoPath::from_git_bytes(b"src/bad-\x80.rs").unwrap();
        let second = RepoPath::from_git_bytes(b"src/bad-\x81.rs").unwrap();
        assert_eq!(first.display(), "src/bad-\\x80.rs");
        assert_eq!(second.display(), "src/bad-\\x81.rs");
        assert_ne!(first.display(), second.display());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_non_utf8_git_paths_on_windows() {
        let error = RepoPath::from_git_bytes(b"invalid-\x80").unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8 on Windows"));
    }
}
