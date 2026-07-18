pub mod diagnostics;
pub mod format;
pub mod lint;

use std::fs;
use std::path::{Component, Path, PathBuf};

/// Discover Aziky source files in a host-independent order.
pub fn discover_sources(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        if path.extension().is_some_and(|extension| extension == "azk") {
            return fs::canonicalize(path)
                .map(|path| vec![path])
                .map_err(|error| format!("failed to resolve '{}': {error}", path.display()));
        }
        return Err(format!("'{}' is not an .azk source file", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("source path '{}' does not exist", path.display()));
    }
    let root = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve '{}': {error}", path.display()))?;
    let mut files = Vec::new();
    collect_sources(&root, &mut files)?;
    files.sort_by(|left, right| portable_path(left).cmp(&portable_path(right)));
    Ok(files)
}

fn collect_sources(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read '{}': {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate '{}': {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "tooling source discovery rejects symbolic link '{}'",
                path.display()
            ));
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            if matches!(name.to_str(), Some(".git" | ".aziky" | "target")) {
                continue;
            }
            collect_sources(&path, files)?;
        } else if file_type.is_file()
            && path.extension().is_some_and(|extension| extension == "azk")
        {
            files.push(path);
        }
    }
    Ok(())
}

pub fn portable_path(path: &Path) -> String {
    let mut prefix = String::new();
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(value) => prefix.push_str(&value.as_os_str().to_string_lossy()),
            Component::RootDir => prefix.push('/'),
            Component::CurDir => {}
            Component::ParentDir => parts.push("..".to_string()),
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
        }
    }
    let joined = parts.join("/");
    if prefix.is_empty() {
        joined
    } else if joined.is_empty() {
        prefix
    } else if prefix.ends_with('/') {
        format!("{prefix}{joined}")
    } else {
        format!("{prefix}/{joined}")
    }
}

pub fn display_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(portable_path)
        .unwrap_or_else(|_| portable_path(path))
}
