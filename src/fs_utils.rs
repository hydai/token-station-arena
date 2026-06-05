use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Returns whether `path` exists.
pub fn path_exists(path: impl AsRef<Path>) -> bool {
    path.as_ref().exists()
}

/// Creates `path` and all missing parents.
pub fn ensure_dir(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    std::fs::create_dir_all(path).with_context(|| format!("create dir {}", path.display()))
}

/// Reads a UTF-8 file to a string.
pub fn read_text(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

/// Reads and deserializes a JSON file.
pub fn read_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let path = path.as_ref();
    let text = read_text(path)?;
    serde_json::from_str(&text).with_context(|| format!("parse JSON {}", path.display()))
}

/// Writes `content` to `path`, creating parent directories as needed.
pub fn write_text(path: impl AsRef<Path>, content: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

/// Serializes `value` as pretty JSON with a trailing newline and writes it.
pub fn write_json<T: Serialize>(path: impl AsRef<Path>, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value).context("serialize JSON")?;
    write_text(path, &format!("{json}\n"))
}

/// Recursively copies the contents of `source` into `destination`.
pub fn copy_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<()> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.with_context(|| format!("walk {}", source.display()))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .expect("walked path is under source");
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("create dir {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create dir {}", parent.display()))?;
            }
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copy to {}", target.display()))?;
        }
    }
    Ok(())
}

/// Recursively finds every file named `file_name` under `dir`.
pub fn find_files(dir: impl AsRef<Path>, file_name: &str) -> Result<Vec<PathBuf>> {
    let mut matches = Vec::new();
    for entry in walkdir::WalkDir::new(dir.as_ref())
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && entry.file_name() == file_name {
            matches.push(entry.into_path());
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_text_creates_parents_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/file.txt");
        write_text(&path, "hello world").unwrap();
        assert!(path_exists(&path));
        assert_eq!(read_text(&path).unwrap(), "hello world");
    }

    #[test]
    fn write_json_is_pretty_with_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value.json");
        let value = serde_json::json!({"b": 2, "a": 1});
        write_json(&path, &value).unwrap();
        let raw = read_text(&path).unwrap();
        assert!(raw.ends_with("}\n"), "should end with newline: {raw:?}");
        assert!(
            raw.contains("\n  \"a\""),
            "should be 2-space indented: {raw:?}"
        );
        let back: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(back["a"], 1);
    }

    #[test]
    fn path_exists_is_false_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!path_exists(dir.path().join("nope")));
    }

    #[test]
    fn copy_dir_recursively_copies_files_and_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        write_text(source.join("a.txt"), "A").unwrap();
        write_text(source.join("sub/b.txt"), "B").unwrap();
        let destination = dir.path().join("dst");

        copy_dir(&source, &destination).unwrap();

        assert_eq!(read_text(destination.join("a.txt")).unwrap(), "A");
        assert_eq!(read_text(destination.join("sub/b.txt")).unwrap(), "B");
    }

    #[test]
    fn find_files_locates_nested_matches_only() {
        let dir = tempfile::tempdir().unwrap();
        write_text(dir.path().join("a/result.json"), "{}").unwrap();
        write_text(dir.path().join("b/c/result.json"), "{}").unwrap();
        write_text(dir.path().join("b/other.txt"), "x").unwrap();
        let found = find_files(dir.path(), "result.json").unwrap();
        assert_eq!(found.len(), 2, "found: {found:?}");
        assert!(found.iter().all(|p| p.ends_with("result.json")));
    }
}
