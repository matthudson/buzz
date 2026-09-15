use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{MAX_RUNTIME_BYTES, MAX_RUNTIME_DEPTH, MAX_RUNTIME_ENTRIES, MAX_RUNTIME_FILES};

pub(super) fn read_bounded_file(path: &Path, maximum: u64, error: &str) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path).map_err(|_| error.to_string())?;
    if metadata.len() == 0 || metadata.len() > maximum {
        return Err(error.to_string());
    }
    let mut file = std::fs::File::open(path).map_err(|_| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error.to_string())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > maximum {
        return Err(error.to_string());
    }
    Ok(bytes)
}

pub(super) fn count_runtime_entry(entries_seen: &mut usize) -> Result<(), String> {
    *entries_seen = entries_seen
        .checked_add(1)
        .filter(|count| *count <= MAX_RUNTIME_ENTRIES)
        .ok_or_else(|| "vyzr_workspace_runtime_unsafe".to_string())?;
    Ok(())
}

fn collect_runtime_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
    entries_seen: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_RUNTIME_DEPTH {
        return Err("vyzr_workspace_runtime_unsafe".to_string());
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|_| "vyzr_workspace_runtime_unavailable".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "vyzr_workspace_runtime_unavailable".to_string())?;
        count_runtime_entry(entries_seen)?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "vyzr_workspace_runtime_unsafe".to_string())?;
        if relative
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == ".git")
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "vyzr_workspace_runtime_unavailable".to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("vyzr_workspace_runtime_unsafe".to_string());
        }
        if metadata.is_dir() {
            collect_runtime_files(root, &path, files, entries_seen, depth + 1)?;
        } else if metadata.is_file() {
            files.push(path);
            if files.len() > MAX_RUNTIME_FILES {
                return Err("vyzr_workspace_runtime_unsafe".to_string());
            }
        } else {
            return Err("vyzr_workspace_runtime_unsafe".to_string());
        }
    }
    Ok(())
}

pub(super) fn runtime_tree_digest(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let mut entries_seen = 0;
    collect_runtime_files(root, root, &mut files, &mut entries_seen, 0)?;
    files.sort_by_key(|path| {
        path.strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .map(|relative| relative.replace('\\', "/"))
            .unwrap_or_default()
    });
    let mut total = 0_u64;
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "vyzr_workspace_runtime_unsafe".to_string())?
            .to_str()
            .ok_or_else(|| "vyzr_workspace_runtime_unsafe".to_string())?
            .replace('\\', "/");
        let metadata = std::fs::metadata(&path)
            .map_err(|_| "vyzr_workspace_runtime_unavailable".to_string())?;
        total = total
            .checked_add(metadata.len())
            .filter(|value| *value <= MAX_RUNTIME_BYTES)
            .ok_or_else(|| "vyzr_workspace_runtime_unsafe".to_string())?;
        let bytes =
            std::fs::read(&path).map_err(|_| "vyzr_workspace_runtime_unavailable".to_string())?;
        if bytes.len() as u64 != metadata.len() {
            return Err("vyzr_workspace_runtime_changed".to_string());
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}
