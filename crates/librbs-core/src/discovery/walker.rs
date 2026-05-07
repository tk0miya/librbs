use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Walk `dir` recursively and return the sorted list of `.rbs` files.
///
/// Mirrors `RBS::FileFinder.each_file`. When `skip_hidden` is true, paths
/// whose any directory component (other than the root) starts with `_`
/// are excluded.
pub(crate) fn find_rbs_files(dir: &Path, skip_hidden: bool) -> std::io::Result<Vec<PathBuf>> {
    if dir.is_file() {
        return Ok(vec![dir.to_path_buf()]);
    }
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                if let Some(io) = err.io_error() {
                    return Err(std::io::Error::new(io.kind(), err.to_string()));
                }
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().map(|e| e != "rbs").unwrap_or(true) {
            continue;
        }
        if skip_hidden {
            let rel = path.strip_prefix(dir).unwrap_or(path);
            let mut hidden = false;
            // Iterate on parent components of the relative path (excluding
            // the file basename itself), matching Ruby's
            // `child.relative_path_from(path).ascend.drop(1)`.
            if let Some(parent) = rel.parent() {
                for comp in parent.components() {
                    let s = comp.as_os_str().to_string_lossy();
                    if s.starts_with('_') {
                        hidden = true;
                        break;
                    }
                }
            }
            if hidden {
                continue;
            }
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_rbs_files() {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../vendor/rbs/core");
        let files = find_rbs_files(&path, false).unwrap();
        assert!(
            files.len() > 50,
            "expected many .rbs files, got {}",
            files.len()
        );
        for f in &files {
            assert_eq!(f.extension().and_then(|s| s.to_str()), Some("rbs"));
        }
    }
}
