use std::collections::BTreeSet;
use std::path::PathBuf;

pub mod repository;
pub mod walker;

pub use repository::{Repository, Version};

/// `SourceTag` records core / library / user-dir distinctions, mirroring
/// the Ruby `source` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTag {
    Core,
    Library {
        name: String,
        version: Option<String>,
    },
    Dir(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Default)]
pub struct Loader {
    pub core_root: Option<PathBuf>,
    pub repository: Repository,
    pub libs: Vec<Library>,
    pub dirs: Vec<PathBuf>,
}

impl Loader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_core_root(core_root: impl Into<PathBuf>) -> Self {
        Self {
            core_root: Some(core_root.into()),
            ..Default::default()
        }
    }

    pub fn add_dir(&mut self, dir: impl Into<PathBuf>) {
        self.dirs.push(dir.into());
    }

    pub fn add_library(&mut self, name: impl Into<String>, version: Option<String>) {
        self.libs.push(Library {
            name: name.into(),
            version,
        });
    }

    /// Resolve every configured source root into a `(tag, path)` list.
    ///
    /// Mirrors `RBS::EnvironmentLoader#each_dir`: walks `core_root`, then
    /// each library via `Repository::lookup`, then user-supplied dirs.
    fn resolve_dirs(&mut self) -> Result<Vec<(SourceTag, PathBuf)>, crate::error::Error> {
        let mut out = Vec::new();
        if let Some(root) = self.core_root.clone() {
            out.push((SourceTag::Core, root));
        }
        // Clone libs first to avoid borrow conflicts with repository.
        let libs = self.libs.clone();
        for lib in &libs {
            match self.repository.lookup(&lib.name, lib.version.as_deref()) {
                Some(p) => out.push((
                    SourceTag::Library {
                        name: lib.name.clone(),
                        version: lib.version.clone(),
                    },
                    p,
                )),
                None => {
                    return Err(crate::error::Error::UnknownLibrary {
                        name: lib.name.clone(),
                    });
                }
            }
        }
        for d in &self.dirs {
            out.push((SourceTag::Dir(d.clone()), d.clone()));
        }
        Ok(out)
    }

    /// Equivalent of `EnvironmentLoader#each_signature` discovery phase.
    /// Returns the deduplicated list of (tag, path) entries to parse.
    pub fn discover_files(&mut self) -> Result<Vec<(SourceTag, PathBuf)>, crate::error::Error> {
        let dirs = self.resolve_dirs()?;
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        let mut out = Vec::new();
        for (tag, dir) in dirs {
            let skip_hidden = !matches!(&tag, SourceTag::Dir(_));
            let files =
                walker::find_rbs_files(&dir, skip_hidden).map_err(|e| crate::error::Error::Io {
                    path: dir.clone(),
                    source: e,
                })?;
            for path in files {
                if seen.insert(path.clone()) {
                    out.push((tag.clone(), path));
                }
            }
        }
        Ok(out)
    }
}
