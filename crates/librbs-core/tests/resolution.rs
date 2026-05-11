//! M3b integration tests: resolution driver.
//!
//! Each test builds a small `Environment` from an inline RBS source and
//! exercises one slice of the M3b acceptance criteria:
//!
//! * the per-`DeclRef` `Vec<ResolvedRef>` slice contains the expected
//!   resolution outcomes in pre-order;
//! * `# resolve-type-names: false` short-circuits resolution for that
//!   source.
//!
//! Canonical-dump fixtures are intentionally absent: the Rust-side
//! `canonical_dump` was deferred to a followup. Compatibility checks
//! happen on the Ruby side from M3c onward.

use std::path::PathBuf;

use librbs_core::env::insert::insert_rbs_source;
use librbs_core::env::resolution::ResolvedRef;
use librbs_core::env::{DeclRef, Environment};
use librbs_core::resolver::driver::resolve;
use librbs_core::source::Source;

fn build_env(rbs_sources: &[&str]) -> Environment {
    let mut env = Environment::new();
    for src in rbs_sources.iter() {
        let path = PathBuf::from(format!("/tmp/{}.rbs", env.sources.len()));
        let s = Source::new(path, src.to_string()).unwrap();
        insert_rbs_source(&mut env, s.parser.signature()).unwrap();
        env.sources.push(s);
    }
    env
}

#[test]
fn resolves_super_class_to_absolute_name() {
    // `class Foo` and `class Bar < Foo` — the `Foo` reference inside the
    // super-class clause resolves to `::Foo`. The resolution side-table
    // therefore holds one slice for `Bar`'s `DeclRef` containing one
    // `Resolved(::Foo)` entry; `Foo`'s own `DeclRef` has no slice
    // because `Foo` has no super-class and no other type-name references.
    let mut env = build_env(&[r#"class Foo end
class Bar < Foo end
"#]);
    let resolution = resolve(&mut env, None);

    assert_eq!(resolution.len(), 1);
    let bar_decl = DeclRef {
        source_index: 0,
        decl_index: 1,
    };
    let slice = resolution
        .get(bar_decl)
        .expect("Bar's super-class reference should be in the resolution");
    assert_eq!(slice.len(), 1);
    let foo_sym = env
        .decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Foo")
        .copied()
        .unwrap();
    assert_eq!(slice[0], ResolvedRef::Resolved(foo_sym));
}

#[test]
fn unknown_super_class_is_recorded_as_unresolved() {
    let mut env = build_env(&[r#"class Foo < Unknown end"#]);
    let resolution = resolve(&mut env, None);

    let foo_decl = DeclRef {
        source_index: 0,
        decl_index: 0,
    };
    let slice = resolution.get(foo_decl).unwrap();
    assert_eq!(slice.len(), 1);
    match slice[0] {
        ResolvedRef::Unresolved(sym) => {
            assert_eq!(env.interner.to_string(sym), "Unknown");
        }
        other => panic!("expected Unresolved, got {:?}", other),
    }
}

#[test]
fn magic_comment_disables_resolution_for_that_source() {
    // First source carries the magic comment; second does not. Only the
    // second source's references should appear in the resolution table.
    let mut env = build_env(&[
        "# resolve-type-names: false\nclass A end\nclass B < A end\n",
        "class C end\nclass D < C end\n",
    ]);
    let resolution = resolve(&mut env, None);

    assert!(
        resolution.iter().all(|(dr, _)| dr.source_index == 1),
        "no entries should come from source 0; got {:?}",
        resolution.iter().map(|(dr, _)| dr).collect::<Vec<_>>()
    );
    assert_eq!(resolution.len(), 1);
}

#[test]
fn only_filter_resolves_named_decl_only() {
    // Two top-level classes that each reference an absolute name. With
    // `only` set to `{::Bar}`, only `Bar`'s super-class reference should
    // appear in the resolution table — `Foo`'s body is skipped.
    let mut env = build_env(&[r#"class Base end
class Foo < Base end
class Bar < Base end
"#]);

    let bar_sym = env
        .decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Bar")
        .copied()
        .unwrap();
    let mut only = rustc_hash::FxHashSet::default();
    only.insert(bar_sym);

    let resolution = resolve(&mut env, Some(&only));

    let base_sym = env
        .decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Base")
        .copied()
        .unwrap();
    let resolved: Vec<ResolvedRef> = resolution
        .iter()
        .flat_map(|(_, slice)| slice.iter().copied())
        .collect();
    assert_eq!(resolved.len(), 1, "only Bar's super-class should resolve");
    assert_eq!(resolved[0], ResolvedRef::Resolved(base_sym));
}
