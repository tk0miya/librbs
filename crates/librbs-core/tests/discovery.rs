use std::path::PathBuf;

use librbs_core::Environment;
use librbs_core::discovery::{Loader, Repository};

fn vendor_rbs() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../vendor/rbs");
    p
}

#[test]
fn discovery_walks_core_dir() {
    let mut loader = Loader::with_core_root(vendor_rbs().join("core"));
    let files = loader.discover_files().unwrap();
    assert!(
        files.len() >= 80,
        "expected ~86 .rbs files, got {}",
        files.len()
    );
    for (_tag, p) in &files {
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("rbs"));
    }
}

#[test]
fn parses_every_core_file() {
    let mut loader = Loader::with_core_root(vendor_rbs().join("core"));
    let env = Environment::from_loader(&mut loader).unwrap();
    assert!(!env.sources.is_empty());
    assert!(
        env.class_decls.len() > 30,
        "got {} class_decls",
        env.class_decls.len()
    );
}

#[test]
fn loads_core_plus_stdlib() {
    let mut repo = Repository::default();
    repo.add(vendor_rbs().join("stdlib"));
    let mut loader = Loader {
        core_root: Some(vendor_rbs().join("core")),
        repository: repo,
        libs: Vec::new(),
        dirs: Vec::new(),
    };
    // Add a few well-known stdlib libraries to mirror what RBS does on
    // a default load.
    for name in ["stringio", "json", "pathname"] {
        loader.add_library(name, None);
    }
    let env = Environment::from_loader(&mut loader).unwrap();
    let count = env.class_decls.len();
    assert!(count > 100, "expected >100 class_decls, got {count}");
}

#[test]
fn resolves_full_core_environment() {
    use librbs_core::resolver::driver::resolve;

    let mut loader = Loader::with_core_root(vendor_rbs().join("core"));
    let mut env = Environment::from_loader(&mut loader).unwrap();
    let res = resolve(&mut env);
    // The full core has thousands of type-name occurrences. We only
    // assert a coarse lower bound — exact byte-level compatibility is
    // verified on the Ruby side from M3c onward.
    assert!(res.len() > 1000, "got {} resolutions", res.len());
    assert!(
        env.class_decls
            .keys()
            .any(|k| env.interner.to_string(*k) == "::Object"),
        "expected ::Object entry to be present"
    );
}

#[test]
fn deterministic_counts() {
    let count_run = || {
        let mut loader = Loader::with_core_root(vendor_rbs().join("core"));
        Environment::from_loader(&mut loader)
            .unwrap()
            .class_decls
            .len()
    };
    let a = count_run();
    let b = count_run();
    assert_eq!(a, b);
}
