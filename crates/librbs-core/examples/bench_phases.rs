//! Phase-by-phase timing for `Environment::from_loader`.
//!
//! Splits the load pipeline into:
//!   1. discover_files       (walk directory tree)
//!   2. read+parse           (parallel, current behavior)
//!   3. insert (serial)      (current behavior)
//!
//! For comparison, also reports a "read-only / read+parse-only" baseline so
//! we can see the cost of parse vs IO. Run with `--release`.
//!
//! Usage:
//!   cargo run --release --example bench_phases -- <repeats>
//!
//! Loads vendored core sigs and a representative slice of stdlib so the
//! footprint roughly matches the `medium` benchmark size.

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

use librbs_core::discovery::{Loader, SourceTag};
use librbs_core::env::{Environment, insert};
use librbs_core::error::{Error, Result};
use librbs_core::source::Source;

const MEDIUM_LIBS: &[&str] = &[
    "pathname", "date", "time", "uri", "optparse", "logger", "stringio", "strscan",
];

fn make_loader(size: &str) -> Loader {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let core_root = repo_root.join("vendor/rbs/core");
    let stdlib_root = repo_root.join("vendor/rbs/stdlib");
    let mut loader = Loader::with_core_root(core_root);
    match size {
        "small" => {}
        "medium" => {
            for lib in MEDIUM_LIBS {
                loader.add_dir(stdlib_root.join(lib));
            }
        }
        "large" => {
            // Every vendored stdlib library.
            if let Ok(rd) = std::fs::read_dir(&stdlib_root) {
                let mut entries: Vec<PathBuf> = rd
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.path())
                    .collect();
                entries.sort();
                for p in entries {
                    loader.add_dir(p);
                }
            }
        }
        _ => panic!("unknown size: {}", size),
    }
    loader
}

fn read_and_parse(files: &[(SourceTag, PathBuf)]) -> Result<Vec<Source>> {
    files
        .par_iter()
        .map(|(tag, path)| -> Result<Source> {
            let content = std::fs::read_to_string(path).map_err(|e| Error::Io {
                path: path.clone(),
                source: e,
            })?;
            let content = content
                .strip_prefix('\u{FEFF}')
                .map(|s| s.to_string())
                .unwrap_or(content);
            Source::new(tag.clone(), path.clone(), content)
                .map_err(|message| Error::Parse { path: path.clone(), message })
        })
        .collect()
}

fn read_only(files: &[(SourceTag, PathBuf)]) -> Result<Vec<String>> {
    files
        .par_iter()
        .map(|(_, path)| -> Result<String> {
            std::fs::read_to_string(path).map_err(|e| Error::Io {
                path: path.clone(),
                source: e,
            })
        })
        .collect()
}

fn insert_serial(sources: &[Source]) -> Result<Environment> {
    let mut env = Environment::new();
    for src in sources {
        insert::insert_rbs_source(&mut env, src.parser.signature())?;
    }
    Ok(env)
}

#[derive(Debug, Default, Clone)]
struct Times {
    discover_ms: f64,
    read_only_ms: f64,
    parse_ms: f64,
    insert_ms: f64,
    total_ms: f64,
    files: usize,
    bytes: usize,
}

fn run_once(size: &str) -> Result<Times> {
    let mut loader = make_loader(size);

    let t0 = Instant::now();
    let files = loader.discover_files()?;
    let discover_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // read-only pass (cold-ish, but OS may cache; we still report for relative scale)
    let t1 = Instant::now();
    let bufs = read_only(&files)?;
    let read_only_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let bytes: usize = bufs.iter().map(|s| s.len()).sum();
    drop(bufs);

    // read+parse (the actual phase used by from_loader)
    let t2 = Instant::now();
    let sources = read_and_parse(&files)?;
    let parse_ms = t2.elapsed().as_secs_f64() * 1000.0;

    // insert (serial)
    let t3 = Instant::now();
    let env = insert_serial(&sources)?;
    let insert_ms = t3.elapsed().as_secs_f64() * 1000.0;

    let total_ms = discover_ms + parse_ms + insert_ms;

    // Touch env so the optimizer can't drop work.
    std::hint::black_box(&env);

    Ok(Times {
        discover_ms,
        read_only_ms,
        parse_ms,
        insert_ms,
        total_ms,
        files: files.len(),
        bytes,
    })
}

fn report(size: &str, repeats: usize) -> Result<()> {
    let warm = run_once(size)?;
    eprintln!(
        "[{}] warmup: files={} bytes={:.1} KiB",
        size,
        warm.files,
        warm.bytes as f64 / 1024.0,
    );
    let mut runs: Vec<Times> = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        runs.push(run_once(size)?);
    }
    let min_of = |f: fn(&Times) -> f64| runs.iter().map(f).fold(f64::INFINITY, f64::min);

    println!();
    println!("== size = {} ==  files={}  ({} repeats, reporting min)",
             size, warm.files, repeats);
    println!("{:-<60}", "");
    let phase = |name: &str, f: fn(&Times) -> f64| {
        println!("  {:<14} {:>8.2} ms", name, min_of(f));
    };
    phase("discover", |t| t.discover_ms);
    phase("read-only", |t| t.read_only_ms);
    phase("read+parse", |t| t.parse_ms);
    phase("insert", |t| t.insert_ms);
    phase("total", |t| t.total_ms);

    let parse_min = min_of(|t| t.parse_ms);
    let insert_min = min_of(|t| t.insert_ms);
    let pipeline_min = parse_min + insert_min;
    println!(
        "  insert / (parse+insert) = {:.1}%",
        100.0 * insert_min / pipeline_min,
    );
    Ok(())
}

fn main() -> Result<()> {
    let repeats: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    for size in ["small", "medium", "large"] {
        report(size, repeats)?;
    }
    Ok(())
}
