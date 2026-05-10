//! Sampling profiler over the serial `insert` phase.
//!
//! Pre-parses every source (so the parse phase isn't sampled), starts a
//! `pprof` sampling profiler at 1 kHz, runs `insert_rbs_source` over
//! every source many times to gather enough samples, then writes a
//! flamegraph SVG.
//!
//! Usage:
//!   cargo run --release --example profile_insert -- [size] [iterations]
//!
//! `size` is one of small / medium / large (default: large)
//! `iterations` controls how many times the insert pass is repeated
//! (default: 200) — more iterations = more samples = sharper flamegraph.

use std::fs::File;
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

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let size = args.next().unwrap_or_else(|| "large".to_string());
    let iterations: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    let mut loader = make_loader(&size);
    let files = loader.discover_files()?;
    let sources = read_and_parse(&files)?;
    eprintln!(
        "size={size}  files={}  iterations={iterations}",
        sources.len(),
    );

    // Calibrate: how long does one pass take?
    let t0 = Instant::now();
    {
        let mut env = Environment::new();
        for src in &sources {
            insert::insert_rbs_source(&mut env, src.parser.signature())?;
        }
        std::hint::black_box(&env);
    }
    let one_pass_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("one insert pass ≈ {:.2} ms; total ≈ {:.2} s",
              one_pass_ms, one_pass_ms * iterations as f64 / 1000.0);

    // Start sampling profiler.
    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(1000)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .expect("profiler start");

    let t1 = Instant::now();
    for _ in 0..iterations {
        let mut env = Environment::new();
        for src in &sources {
            insert::insert_rbs_source(&mut env, src.parser.signature())?;
        }
        std::hint::black_box(&env);
    }
    let elapsed = t1.elapsed().as_secs_f64();
    eprintln!("profiled {} iterations in {:.2} s", iterations, elapsed);

    let report = guard.report().build().expect("report");
    let out = std::env::current_dir().unwrap().join(format!("flamegraph_insert_{size}.svg"));
    let f = File::create(&out).expect("create svg");
    report.flamegraph(f).expect("write flamegraph");
    eprintln!("wrote {}", out.display());

    use std::collections::HashMap;
    let mut by_leaf: HashMap<String, usize> = HashMap::new();
    let mut by_inclusive: HashMap<String, usize> = HashMap::new();
    let mut by_category: HashMap<&'static str, usize> = HashMap::new();
    let mut total_samples: usize = 0;
    for (frames, count) in report.data.iter() {
        total_samples += *count as usize;
        if let Some(top) = frames.frames.first().and_then(|f| f.first()) {
            *by_leaf.entry(top.name()).or_insert(0) += *count as usize;
        }
        // Inclusive: count each unique function name once per stack.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for inner in &frames.frames {
            for sym in inner {
                let name = sym.name();
                if seen.insert(name.clone()) {
                    *by_inclusive.entry(name).or_insert(0) += *count as usize;
                }
            }
        }
        // Category: pick first matching frame from the stack.
        let mut cat = "other";
        'cat: for inner in &frames.frames {
            for sym in inner {
                let n = sym.name();
                cat = if n.contains("sip::") || n.contains("SipHasher") {
                    "hash:sip"
                } else if n.contains("hashbrown::") || n.contains("RawTable") {
                    "hashbrown"
                } else if n.contains("librbs_core::interner") {
                    "interner"
                } else if n.contains("librbs_core::env::insert::intern") {
                    "insert::intern_*"
                } else if n.contains("librbs_core::env::insert::insert_decl") {
                    "insert::insert_decl"
                } else if n.contains("ruby_rbs::node") {
                    "ruby_rbs ast"
                } else if n.contains("alloc::") || n.contains("__rust_alloc") {
                    "alloc"
                } else {
                    continue;
                };
                break 'cat;
            }
        }
        *by_category.entry(cat).or_insert(0) += *count as usize;
    }
    let print_top = |label: &str, map: &HashMap<String, usize>, n: usize| {
        let mut rows: Vec<(&String, &usize)> = map.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        println!("\n{label} (top {n}):");
        println!("{:-<80}", "");
        for (name, count) in rows.iter().take(n) {
            let pct = 100.0 * (**count as f64) / (total_samples as f64);
            println!("  {:>6.2}%  {}", pct, name);
        }
    };
    println!("\n== {total_samples} total samples ==");
    print_top("Top self-time leaves", &by_leaf, 20);
    print_top("Top inclusive (any frame)", &by_inclusive, 25);

    let mut cats: Vec<(&&'static str, &usize)> = by_category.iter().collect();
    cats.sort_by(|a, b| b.1.cmp(a.1));
    println!("\nBy category (first matching frame on each stack):");
    println!("{:-<80}", "");
    for (cat, count) in cats {
        let pct = 100.0 * (*count as f64) / (total_samples as f64);
        println!("  {:>6.2}%  {}", pct, cat);
    }

    Ok(())
}
