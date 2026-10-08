//! The filesystem the compiler reads stylesheets from.
//!
//! Every read during compilation (the entry file, `@use`/`@forward`/`@import` targets) goes through
//! [`FileSystem`], so the compiler can run against the real disk ([`OsFs`]) or files held in memory
//! ([`MemoryFs`], used by the web playground and tests).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

pub trait FileSystem: fmt::Debug {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn is_file(&self, path: &Path) -> bool;
    /// The path that identifies the file, so one file reached two ways is loaded once.
    fn canonicalize(&self, path: &Path) -> PathBuf;
}

/// The real filesystem.
#[derive(Debug, Default)]
pub struct OsFs;

impl FileSystem for OsFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn canonicalize(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Files held in memory, keyed by normalized path (`a/./b/../c.scss` → `a/c.scss`).
#[derive(Debug, Default)]
pub struct MemoryFs {
    files: HashMap<PathBuf, String>,
}

impl MemoryFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl AsRef<Path>, contents: impl Into<String>) {
        self.files.insert(normalize(path.as_ref()), contents.into());
    }
}

impl FileSystem for MemoryFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(&normalize(path))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(&normalize(path))
    }

    fn canonicalize(&self, path: &Path) -> PathBuf {
        normalize(path)
    }
}

/// Resolves `.` and `..` lexically (there are no symlinks to follow in memory).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), None | Some(Component::ParentDir)) {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_paths_are_normalized() {
        let mut fs = MemoryFs::new();
        fs.insert("styles/./_theme.scss", "$a: 1;");
        assert!(fs.is_file(Path::new("styles/components/../_theme.scss")));
        assert_eq!(fs.read_to_string(Path::new("styles/_theme.scss")).unwrap(), "$a: 1;");
        assert!(!fs.is_file(Path::new("_theme.scss")));
        assert_eq!(fs.canonicalize(Path::new("./a/b/../c.scss")), PathBuf::from("a/c.scss"));
    }
}
