//! Filesystem walker that replaces upstream `RBS::FileFinder.each_file`.
//!
//! Replicates `vendor/rbs/lib/rbs/file_finder.rb`:
//!
//! - If a spec's path is a regular file, it is yielded directly.
//! - If it is a directory, it is walked recursively for `*.rbs` files;
//!   `skip_hidden` filters any file whose ancestor directory (between
//!   the spec root and the file's parent, exclusive of the file itself)
//!   has a basename starting with `_`.
//! - Within a single spec, results are sorted by their path string —
//!   matching upstream's `paths.sort_by!(&:to_s)` after `Pathname.glob`.
//! - Across specs, paths are concatenated in the order specs are given
//!   and deduplicated keeping the first occurrence, matching the
//!   `each_signature` loop's `files = Set[]` dedup.
//!
//! Walking is done with rayon across specs; each spec walks serially.
//! Most of the wall time in upstream `from_loader` came from
//! `Dir.glob` + ancestor-prefix filtering, both of which the Rust
//! version expresses as a single readdir-driven recursion that prunes
//! `_`-prefixed directories on descent instead of post-filtering.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rustc_hash::FxHashSet;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct DirSpec {
    pub path: PathBuf,
    pub skip_hidden: bool,
}

/// Discover `.rbs` files across the given spec list.
///
/// Per-spec results are sorted to match upstream's `Pathname.glob` +
/// `sort_by(&:to_s)`. The cross-spec concatenation is deduplicated by
/// first-occurrence, matching `RBS::EnvironmentLoader#each_signature`.
pub fn discover_rbs_files(specs: Vec<DirSpec>) -> Result<Vec<PathBuf>> {
    let per_spec: Vec<Result<Vec<PathBuf>>> = specs
        .par_iter()
        .map(|spec| walk_spec(&spec.path, spec.skip_hidden))
        .collect();

    let mut seen: FxHashSet<PathBuf> = FxHashSet::default();
    let mut out: Vec<PathBuf> = Vec::new();
    for result in per_spec {
        for path in result? {
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn walk_spec(root: &Path, skip_hidden: bool) -> Result<Vec<PathBuf>> {
    let meta = match std::fs::metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::Io {
                path: root.to_path_buf(),
                source: e,
            });
        }
    };
    if meta.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !meta.is_dir() {
        return Ok(Vec::new());
    }

    let mut out: Vec<PathBuf> = Vec::new();
    walk_dir(root, skip_hidden, &mut out)?;
    out.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    Ok(out)
}

fn walk_dir(dir: &Path, skip_hidden: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(Error::Io {
                path: dir.to_path_buf(),
                source: e,
            });
        }
    };

    for entry in entries {
        let entry = entry.map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let file_type = entry.file_type().map_err(|e| Error::Io {
            path: entry.path(),
            source: e,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Upstream `Pathname.glob("**/*.rbs")` runs with default flags
        // (no `File::FNM_DOTMATCH`), so `*` never matches a leading
        // `.` — neither in the file basename (`.foo.rbs`) nor in any
        // ancestor segment (`.hidden/foo.rbs`). This filter applies
        // regardless of `skip_hidden`; `skip_hidden` only adds the
        // separate `_`-prefix ancestor-dir filter on top. The spec
        // root itself may begin with `.` (e.g.
        // `benchmark/fixtures/.gem_rbs_collection/...` registered via
        // `add_collection`) because the glob pattern includes that
        // segment as a literal — we mirror that by never re-checking
        // the spec root, only its descendants.
        if name.starts_with('.') {
            continue;
        }

        if file_type.is_dir() {
            if skip_hidden && name.starts_with('_') {
                continue;
            }
            walk_dir(&path, skip_hidden, out)?;
        } else if file_type.is_file() {
            if name.ends_with(".rbs") {
                out.push(path);
            }
        }
        // Symlinks: ignored. Upstream `Pathname.glob` with `**` does
        // not follow directory symlinks by default; we mirror that.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(name: &str) -> Self {
            let mut root = std::env::temp_dir();
            root.push(format!(
                "librbs-discovery-{}-{}",
                name,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn touch(&self, rel: &str) -> PathBuf {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, "").unwrap();
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn picks_only_rbs_files() {
        let t = TempTree::new("ext");
        t.touch("a.rbs");
        t.touch("b.rb");
        t.touch("nested/c.rbs");
        t.touch("nested/d.txt");

        let mut got = discover_rbs_files(vec![DirSpec {
            path: t.root.clone(),
            skip_hidden: true,
        }])
        .unwrap();
        got.sort();
        let mut want = vec![t.root.join("a.rbs"), t.root.join("nested/c.rbs")];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn skip_hidden_drops_underscore_dirs_only() {
        let t = TempTree::new("hidden");
        t.touch("_priv/inner.rbs");
        t.touch("pub/inner.rbs");
        t.touch("_top.rbs"); // file with leading _ at top level is kept
        t.touch("pub/_under.rbs"); // file with leading _ at deep level is kept

        let got = discover_rbs_files(vec![DirSpec {
            path: t.root.clone(),
            skip_hidden: true,
        }])
        .unwrap();
        let got_set: std::collections::BTreeSet<_> = got.into_iter().collect();
        let want: std::collections::BTreeSet<_> = vec![
            t.root.join("_top.rbs"),
            t.root.join("pub/inner.rbs"),
            t.root.join("pub/_under.rbs"),
        ]
        .into_iter()
        .collect();
        assert_eq!(got_set, want);
    }

    #[test]
    fn skip_hidden_false_keeps_underscore_dirs() {
        let t = TempTree::new("nohidden");
        t.touch("_priv/inner.rbs");
        t.touch("pub/inner.rbs");

        let got = discover_rbs_files(vec![DirSpec {
            path: t.root.clone(),
            skip_hidden: false,
        }])
        .unwrap();
        let got_set: std::collections::BTreeSet<_> = got.into_iter().collect();
        let want: std::collections::BTreeSet<_> =
            vec![t.root.join("_priv/inner.rbs"), t.root.join("pub/inner.rbs")]
                .into_iter()
                .collect();
        assert_eq!(got_set, want);
    }

    #[test]
    fn dedup_keeps_first_occurrence_order() {
        let t = TempTree::new("dedup");
        t.touch("a.rbs");
        t.touch("b.rbs");

        let got = discover_rbs_files(vec![
            DirSpec {
                path: t.root.clone(),
                skip_hidden: true,
            },
            DirSpec {
                path: t.root.clone(),
                skip_hidden: true,
            },
        ])
        .unwrap();

        // Each spec yields the same two paths sorted; the second
        // spec's entries are all already-seen, so the merged list
        // is just the first spec's sorted output.
        assert_eq!(got, vec![t.root.join("a.rbs"), t.root.join("b.rbs")]);
    }

    #[test]
    fn file_spec_yields_itself() {
        let t = TempTree::new("filespec");
        let single = t.touch("only.rbs");

        let got = discover_rbs_files(vec![DirSpec {
            path: single.clone(),
            skip_hidden: true,
        }])
        .unwrap();
        assert_eq!(got, vec![single]);
    }

    #[test]
    fn within_spec_sorted_by_path_string() {
        let t = TempTree::new("sort");
        t.touch("c.rbs");
        t.touch("a.rbs");
        t.touch("b/x.rbs");

        let got = discover_rbs_files(vec![DirSpec {
            path: t.root.clone(),
            skip_hidden: true,
        }])
        .unwrap();
        assert_eq!(
            got,
            vec![
                t.root.join("a.rbs"),
                t.root.join("b/x.rbs"),
                t.root.join("c.rbs"),
            ]
        );
    }

    #[test]
    fn dotfile_files_are_skipped_regardless_of_skip_hidden() {
        // Upstream `Pathname.glob("**/*.rbs")` excludes leading-`.`
        // basenames because `*` doesn't match a leading `.` by
        // default. That filter is independent of `skip_hidden`.
        for skip in [true, false] {
            let t = TempTree::new(&format!("dotfiles-{}", skip));
            t.touch(".foo.rbs");
            t.touch("ok.rbs");

            let got = discover_rbs_files(vec![DirSpec {
                path: t.root.clone(),
                skip_hidden: skip,
            }])
            .unwrap();
            assert_eq!(
                got,
                vec![t.root.join("ok.rbs")],
                "leading-dot file should be excluded with skip_hidden={}",
                skip
            );
        }
    }

    #[test]
    fn dotted_dirs_are_not_traversed_regardless_of_skip_hidden() {
        for skip in [true, false] {
            let t = TempTree::new(&format!("dotdirs-{}", skip));
            t.touch(".hidden/inner.rbs");
            t.touch(".git/foo.rbs");
            t.touch("normal/ok.rbs");

            let got = discover_rbs_files(vec![DirSpec {
                path: t.root.clone(),
                skip_hidden: skip,
            }])
            .unwrap();
            assert_eq!(
                got,
                vec![t.root.join("normal/ok.rbs")],
                "leading-dot ancestor dirs should be excluded with skip_hidden={}",
                skip
            );
        }
    }

    #[test]
    fn spec_root_with_leading_dot_is_traversed() {
        // The spec root itself may legitimately start with `.` — e.g.
        // collection caches at `benchmark/fixtures/.gem_rbs_collection/...`.
        // Only descendants of the spec root are subject to the dotfile
        // filter; the root is always entered.
        let parent = TempTree::new("dotroot");
        let spec_root = parent.root.join(".inside");
        std::fs::create_dir_all(&spec_root).unwrap();
        let kept = spec_root.join("ok.rbs");
        std::fs::write(&kept, "").unwrap();
        let nested_dot = spec_root.join(".hidden");
        std::fs::create_dir_all(&nested_dot).unwrap();
        std::fs::write(nested_dot.join("inner.rbs"), "").unwrap();

        let got = discover_rbs_files(vec![DirSpec {
            path: spec_root.clone(),
            skip_hidden: true,
        }])
        .unwrap();
        assert_eq!(got, vec![kept]);
    }

    #[test]
    fn missing_dir_is_empty() {
        let mut p = std::env::temp_dir();
        p.push("librbs-discovery-doesnotexist-zzzz");
        let got = discover_rbs_files(vec![DirSpec {
            path: p,
            skip_hidden: true,
        }])
        .unwrap();
        assert!(got.is_empty());
    }
}
