//! Native equivalent of `RBS::Repository` (and its `GemRBS` /
//! `VersionPath` nested types).
//!
//! Upstream walks each registered repository directory listing its
//! immediate children (gem names), then for each gem walks its child
//! directories listing version names. The hot path on a large
//! collection (≈92 gems) was `Pathname#each_child` -> `Dir.open` /
//! `Dir.foreach` running ~250 times during a single
//! `RBS::Environment.from_loader`, accounting for the bulk of the
//! residual `from_loader` time once file discovery moved to Rust.
//!
//! Replicated semantics from `vendor/rbs/lib/rbs/repository.rb`:
//!
//! - A repository "dir" is a directory whose immediate children are
//!   gem-name directories. Multiple repository dirs may contribute
//!   the same gem name; the gem's effective version set is the union
//!   of versions found across all contributing paths.
//! - Each gem-name directory has version-name child directories.
//!   Strings that are not valid `Gem::Version`s are skipped, and
//!   prerelease versions (those containing any non-numeric component
//!   such as `1.0.0.pre`) are also skipped.
//! - When two contributing paths supply the same version for the same
//!   gem, the later-registered path wins (upstream uses `versions[v]
//!   = ...` unconditionally inside the per-path loop).
//! - `lookup(name, version)`: when `version` is `None`, returns the
//!   highest known version's path. When `version` is `Some`, returns
//!   the largest known version `<= requested.release()` — the
//!   `release()` step strips the prerelease tail so e.g. requesting
//!   `1.0.0.pre1` falls back onto `1.0.0`.
//!
//! The version parser is deliberately narrower than `Gem::Version`:
//! upstream's lookup path filters out prereleases up-front, so the
//! version values we ever compare are strictly numeric component
//! sequences (`u64` per dot-separated piece). Trailing-zero
//! equivalence (`1.0 == 1.0.0`) is preserved.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    components: Vec<u64>,
}

impl Version {
    /// Parse a release-only version string (numeric components only,
    /// dot-separated). Returns `None` for empty input or strings
    /// containing any non-numeric component — matches the upstream
    /// filter `Gem::Version.correct?(s) && !Gem::Version.create(s).prerelease?`.
    pub fn parse_release(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let mut components: Vec<u64> = Vec::new();
        for piece in s.split('.') {
            if piece.is_empty() {
                return None;
            }
            let n: u64 = piece.parse().ok()?;
            components.push(n);
        }
        Some(Self::canonical(components))
    }

    /// Strip the prerelease tail. The first non-numeric component
    /// (and everything after it) is discarded; everything before is
    /// kept. Matches `Gem::Version#release` for the cases the
    /// repository lookup cares about: a request like `1.0.0.pre1`
    /// resolves through `release()` to `1.0.0`.
    pub fn parse_request(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let mut components: Vec<u64> = Vec::new();
        for piece in s.split('.') {
            match piece.parse::<u64>() {
                Ok(n) => components.push(n),
                Err(_) => break,
            }
        }
        if components.is_empty() {
            return None;
        }
        Some(Self::canonical(components))
    }

    /// Strip trailing zero components so `1.0`, `1.0.0`, and `1`
    /// all canonicalize to the same value — required for
    /// `PartialEq` / `Hash` to agree with `Gem::Version` semantics
    /// (`Gem::Version.new("1.0") == Gem::Version.new("1.0.0")`).
    /// At least one component is always kept, so `0`, `0.0`,
    /// `0.0.0` all canonicalize to `[0]`.
    fn canonical(mut components: Vec<u64>) -> Self {
        while components.len() > 1 && components.last() == Some(&0) {
            components.pop();
        }
        Self { components }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // Trailing-zero equivalence: walk to max(len(a), len(b)) and
        // treat missing components as zero. Same as Gem::Version's
        // canonical_segments behavior for numeric-only versions.
        let len = self.components.len().max(other.components.len());
        for i in 0..len {
            let a = self.components.get(i).copied().unwrap_or(0);
            let b = other.components.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => continue,
                neq => return neq,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct GemEntry {
    /// Repository roots that contribute this gem name. Populated
    /// eagerly by `add_dir`. The per-version walk is deferred to
    /// `load_versions` — matching upstream's two-step laziness
    /// (`Repository#add` only enumerates gem-name dirs, the per-gem
    /// `GemRBS#load!` only fires on actual `versions` access).
    paths: Vec<PathBuf>,
    /// `Some` once `load_versions` has populated it. Later paths
    /// overwrite earlier ones for the same version, matching
    /// upstream's "last registration wins" inside `GemRBS#load!`.
    versions: Option<FxHashMap<Version, PathBuf>>,
}

impl GemEntry {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            versions: None,
        }
    }

    fn load_versions(&mut self) -> &FxHashMap<Version, PathBuf> {
        if self.versions.is_some() {
            return self.versions.as_ref().unwrap();
        }
        let mut versions: FxHashMap<Version, PathBuf> = FxHashMap::default();
        for gem_path in &self.paths {
            let version_entries = match std::fs::read_dir(gem_path) {
                Ok(it) => it,
                Err(_) => continue,
            };
            for vent in version_entries.flatten() {
                let Ok(vft) = vent.file_type() else { continue };
                if !vft.is_dir() {
                    continue;
                }
                let Ok(vname) = vent.file_name().into_string() else {
                    continue;
                };
                let Some(version) = Version::parse_release(&vname) else {
                    continue;
                };
                versions.insert(version, vent.path());
            }
        }
        self.versions = Some(versions);
        self.versions.as_ref().unwrap()
    }

    fn latest(versions: &FxHashMap<Version, PathBuf>) -> Option<&PathBuf> {
        versions.iter().max_by(|a, b| a.0.cmp(b.0)).map(|(_, p)| p)
    }

    /// Largest version `<= requested`. Falls back to the smallest
    /// known version when nothing is `<= requested` — matches
    /// upstream `Repository.find_best_version`.
    fn best_for<'a>(
        versions: &'a FxHashMap<Version, PathBuf>,
        requested: &Version,
    ) -> Option<&'a PathBuf> {
        let mut sorted: Vec<&Version> = versions.keys().collect();
        sorted.sort();
        let mut hit: Option<&Version> = None;
        for v in sorted.iter().rev() {
            if **v <= *requested {
                hit = Some(*v);
                break;
            }
        }
        let chosen = hit.or_else(|| sorted.first().copied())?;
        versions.get(chosen)
    }
}

#[derive(Debug)]
pub struct RepositoryIndex {
    gems: FxHashMap<String, GemEntry>,
}

impl RepositoryIndex {
    pub fn new() -> Self {
        Self {
            gems: FxHashMap::default(),
        }
    }

    /// Register a repository root. Only the immediate children
    /// (gem-name dirs) are enumerated here — version subdirectories
    /// are walked lazily by `lookup`. This matches upstream's
    /// laziness in `Repository#add` vs `GemRBS#load!`: simply
    /// configuring a repository must not pay the per-gem version
    /// walk that only consumers who actually look the gem up should
    /// trigger. Non-existent dirs and unreadable children are
    /// skipped silently.
    pub fn add_dir(&mut self, repo_root: &Path) {
        let entries = match std::fs::read_dir(repo_root) {
            Ok(it) => it,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let gem_name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let gem_entry = self.gems.entry(gem_name).or_insert_with(GemEntry::new);
            gem_entry.paths.push(entry.path());
        }
    }

    /// Return the path that upstream `Repository#lookup(gem, version)`
    /// would return: best version (or latest when `version` is
    /// `None`). `None` here means "library not in this repository";
    /// the caller decides whether that should raise
    /// `UnknownLibraryError`.
    ///
    /// The first call for a given gem triggers its version-subdir
    /// walk; subsequent calls reuse the cached map.
    pub fn lookup(&mut self, name: &str, version: Option<&str>) -> Option<PathBuf> {
        let gem = self.gems.get_mut(name)?;
        let versions = gem.load_versions();
        if versions.is_empty() {
            return None;
        }
        match version {
            None => GemEntry::latest(versions).cloned(),
            Some(v) => {
                let req = Version::parse_request(v)?;
                GemEntry::best_for(versions, &req).cloned()
            }
        }
    }
}

impl Default for RepositoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempRepo {
        root: PathBuf,
    }

    impl TempRepo {
        fn new(name: &str) -> Self {
            let mut root = std::env::temp_dir();
            root.push(format!(
                "librbs-repo-{}-{}",
                name,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn add(&self, gem: &str, version: &str) -> PathBuf {
            let p = self.root.join(gem).join(version);
            fs::create_dir_all(&p).unwrap();
            p
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn version_parses_and_compares() {
        let v1 = Version::parse_release("1.0").unwrap();
        let v2 = Version::parse_release("1.0.0").unwrap();
        let v3 = Version::parse_release("1.0.1").unwrap();
        assert_eq!(v1, v2);
        assert!(v1 < v3);
        assert!(v3 > v2);
    }

    #[test]
    fn parse_release_rejects_prerelease() {
        assert!(Version::parse_release("1.0.0.pre").is_none());
        assert!(Version::parse_release("1.0.0a").is_none());
        assert!(Version::parse_release("1.0.0-alpha").is_none());
    }

    #[test]
    fn parse_request_strips_prerelease_tail() {
        let v = Version::parse_request("1.0.0.pre1").unwrap();
        assert_eq!(v, Version::parse_release("1.0.0").unwrap());
    }

    #[test]
    fn lookup_latest_when_no_version_requested() {
        let t = TempRepo::new("latest");
        t.add("gem-a", "1.0.0");
        let v2 = t.add("gem-a", "2.0.0");
        t.add("gem-a", "1.5.0");

        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t.root);
        assert_eq!(idx.lookup("gem-a", None), Some(v2));
    }

    #[test]
    fn lookup_best_le_for_specific_version() {
        let t = TempRepo::new("best");
        let v100 = t.add("gem-a", "1.0.0");
        let v110 = t.add("gem-a", "1.1.0");
        let v200 = t.add("gem-a", "2.0.0");

        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t.root);
        assert_eq!(idx.lookup("gem-a", Some("1.0.5")), Some(v100));
        assert_eq!(idx.lookup("gem-a", Some("1.1.0")), Some(v110.clone()));
        assert_eq!(idx.lookup("gem-a", Some("1.5.0")), Some(v110));
        assert_eq!(idx.lookup("gem-a", Some("3.0.0")), Some(v200));
    }

    #[test]
    fn lookup_falls_back_to_smallest_when_request_below_all() {
        let t = TempRepo::new("below");
        let v200 = t.add("gem-a", "2.0.0");
        t.add("gem-a", "3.0.0");

        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t.root);
        assert_eq!(idx.lookup("gem-a", Some("1.0.0")), Some(v200));
    }

    #[test]
    fn prerelease_version_dirs_are_skipped() {
        let t = TempRepo::new("prerelease");
        let v100 = t.add("gem-a", "1.0.0");
        t.add("gem-a", "2.0.0.pre");
        t.add("gem-a", "not-a-version");

        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t.root);
        assert_eq!(idx.lookup("gem-a", None), Some(v100));
    }

    #[test]
    fn unknown_gem_returns_none() {
        let t = TempRepo::new("unknown");
        t.add("gem-a", "1.0.0");
        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t.root);
        assert_eq!(idx.lookup("gem-b", None), None);
    }

    #[test]
    fn missing_repo_root_is_ignored() {
        let mut p = std::env::temp_dir();
        p.push("librbs-repo-doesnotexist-zzzz");
        let mut idx = RepositoryIndex::new();
        idx.add_dir(&p);
        assert_eq!(idx.lookup("gem-a", None), None);
    }

    #[test]
    fn later_dir_overrides_earlier_for_same_version() {
        let t1 = TempRepo::new("dup1");
        let t2 = TempRepo::new("dup2");
        t1.add("gem-a", "1.0.0");
        let v_late = t2.add("gem-a", "1.0.0");

        let mut idx = RepositoryIndex::new();
        idx.add_dir(&t1.root);
        idx.add_dir(&t2.root);
        assert_eq!(idx.lookup("gem-a", None), Some(v_late));
    }
}
