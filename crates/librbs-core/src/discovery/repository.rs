use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A semver-ish version. Mirrors `Gem::Version#release` semantics enough to
/// pick the same best_version for the same input, but does not implement the
/// full `Gem::Version` algebra.
///
/// TODO(followups): Extend to full `Gem::Version` semantics (prerelease
/// segments, release-vs-prerelease comparison, dotted alphanumerics) before
/// we start resolving third-party gem versions in M3+. See
/// `docs/tasks/followups.md` for the tracked item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub segments: Vec<u64>,
}

impl Version {
    /// Parse a version string. Mirrors the subset of `Gem::Version.correct?`
    /// we care about: dot-separated numeric segments. Returns `None` for
    /// strings containing non-numeric or pre-release suffixes.
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let mut segments = Vec::new();
        for part in s.split('.') {
            // Skip pre-releases (anything non-numeric).
            if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            segments.push(part.parse::<u64>().ok()?);
        }
        Some(Self { segments })
    }

    fn cmp_segments(&self, other: &Self) -> std::cmp::Ordering {
        let len = self.segments.len().max(other.segments.len());
        for i in 0..len {
            let a = self.segments.get(i).copied().unwrap_or(0);
            let b = other.segments.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cmp_segments(other)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self.segments.iter().map(|s| s.to_string()).collect();
        f.write_str(&parts.join("."))
    }
}

#[derive(Debug, Clone)]
pub struct VersionPath {
    pub version: Version,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct GemRBS {
    pub name: String,
    paths: Vec<PathBuf>,
    versions: Option<Vec<VersionPath>>,
}

impl GemRBS {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            paths: Vec::new(),
            versions: None,
        }
    }

    pub fn add_path(&mut self, path: impl Into<PathBuf>) {
        self.paths.push(path.into());
        self.versions = None;
    }

    pub fn load(&mut self) {
        let mut versions: Vec<VersionPath> = Vec::new();
        for gem_path in &self.paths {
            let entries = match std::fs::read_dir(gem_path) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();
                if let Some(v) = Version::parse(&name) {
                    let p = gem_path.join(&*name);
                    versions.push(VersionPath {
                        version: v,
                        path: p,
                    });
                }
            }
        }
        versions.sort_by(|a, b| a.version.cmp(&b.version));
        self.versions = Some(versions);
    }

    pub fn versions(&mut self) -> &[VersionPath] {
        if self.versions.is_none() {
            self.load();
        }
        self.versions.as_deref().unwrap_or(&[])
    }

    pub fn latest(&mut self) -> Option<&VersionPath> {
        self.versions().last()
    }

    pub fn find_best(&mut self, target: Option<&Version>) -> Option<&VersionPath> {
        let versions = self.versions();
        if versions.is_empty() {
            return None;
        }
        match target {
            None => versions.last(),
            Some(target) => {
                // largest version that is <= target.
                let mut best: Option<&VersionPath> = None;
                for vp in versions {
                    if vp.version <= *target {
                        best = Some(vp);
                    } else {
                        break;
                    }
                }
                best.or_else(|| versions.first())
            }
        }
    }
}

/// Mirror of `RBS::Repository`.
#[derive(Debug, Default)]
pub struct Repository {
    pub dirs: Vec<PathBuf>,
    pub gems: HashMap<String, GemRBS>,
}

impl Repository {
    /// Create a repository. If `with_default_stdlib` is true, the
    /// `vendor/rbs/stdlib` directory (relative to the repo root) is added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a directory of gem-name subdirs to the repository.
    pub fn add(&mut self, dir: impl Into<PathBuf>) {
        let dir: PathBuf = dir.into();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                self.dirs.push(dir);
                return;
            }
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let gem_name = entry.file_name().to_string_lossy().into_owned();
            let gem = self
                .gems
                .entry(gem_name.clone())
                .or_insert_with(|| GemRBS::new(gem_name));
            gem.add_path(entry.path());
        }
        self.dirs.push(dir);
    }

    pub fn lookup(&mut self, gem: &str, version: Option<&str>) -> Option<PathBuf> {
        let g = self.gems.get_mut(gem)?;
        let v = version.and_then(Version::parse);
        let vp = g.find_best(v.as_ref())?;
        Some(vp.path.clone())
    }
}

/// Returns the default stdlib root, given the path to `vendor/rbs/stdlib`.
pub fn default_stdlib_root(vendor_rbs: &Path) -> PathBuf {
    vendor_rbs.join("stdlib")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.segments, vec![1, 2, 3]);
        assert!(Version::parse("1.0.0.alpha").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn version_orders() {
        let a = Version::parse("1.2").unwrap();
        let b = Version::parse("1.2.0").unwrap();
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        let c = Version::parse("1.10.0").unwrap();
        let d = Version::parse("1.2.0").unwrap();
        assert!(c > d);
    }
}
