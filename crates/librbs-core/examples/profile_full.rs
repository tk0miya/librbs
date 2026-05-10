//! Full-pipeline profile: read + parse + insert, all under the profiler.
//!
//! Companion to `profile_insert.rs`. That one isolates insert by
//! pre-parsing first; this one captures the relative cost of every
//! phase by running `Environment::from_loader`-equivalent work inside
//! the profiler guard.
//!
//! Usage:
//!   cargo run --release --example profile_full -- [size] [iterations]

use std::collections::{HashMap, HashSet};
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
            Source::new(tag.clone(), path.clone(), content).map_err(|message| Error::Parse {
                path: path.clone(),
                message,
            })
        })
        .collect()
}

fn one_pipeline(loader: &mut Loader) -> Result<()> {
    let files = loader.discover_files()?;
    let sources = read_and_parse(&files)?;
    let mut env = Environment::new();
    for src in &sources {
        insert::insert_rbs_source(&mut env, src.parser.signature())?;
    }
    std::hint::black_box(&env);
    std::hint::black_box(&sources);
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let size = args.next().unwrap_or_else(|| "large".to_string());
    let iterations: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(500);

    // Warm FS cache + rayon pool.
    {
        let mut warm = make_loader(&size);
        one_pipeline(&mut warm)?;
    }

    // Calibrate.
    let t0 = Instant::now();
    {
        let mut loader = make_loader(&size);
        one_pipeline(&mut loader)?;
    }
    let one_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "size={size}  one pass ≈ {:.2} ms  iterations={iterations}",
        one_ms
    );

    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(1000)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .expect("profiler start");

    let t1 = Instant::now();
    for _ in 0..iterations {
        let mut loader = make_loader(&size);
        one_pipeline(&mut loader)?;
    }
    let elapsed = t1.elapsed().as_secs_f64();
    eprintln!("profiled {} iterations in {:.2} s", iterations, elapsed);

    let report = guard.report().build().expect("report");
    let out = std::env::current_dir()
        .unwrap()
        .join(format!("flamegraph_full_{size}.svg"));
    let f = File::create(&out).expect("create svg");
    report.flamegraph(f).expect("write flamegraph");
    eprintln!("wrote {}", out.display());

    // Aggregate inclusive samples for high-level phase frames.
    let phase_markers: &[(&str, &str)] = &[
        (
            "insert::insert_rbs_source",
            "librbs_core::env::insert::insert_rbs_source",
        ),
        ("Source::new (parse)", "librbs_core::source::Source::new"),
        (
            "ManagedParser::parse",
            "librbs_core::source::ManagedParser::parse",
        ),
        ("read_to_string", "std::fs::read_to_string"),
        ("rayon par_iter", "rayon::iter"),
        ("walkdir/discover", "librbs_core::discovery"),
    ];

    let mut totals: HashMap<&str, usize> = HashMap::new();
    let mut samples_total: usize = 0;
    for (frames, count) in report.data.iter() {
        samples_total += *count as usize;
        let mut hit: HashSet<&str> = HashSet::new();
        for inner in &frames.frames {
            for sym in inner {
                let n = sym.name();
                for (label, needle) in phase_markers {
                    if n.contains(needle) && hit.insert(*label) {
                        *totals.entry(*label).or_insert(0) += *count as usize;
                    }
                }
            }
        }
    }

    println!("\n== {samples_total} total samples ==");
    println!("\nPipeline phase coverage (inclusive — % of samples that pass through the frame):");
    println!("{:-<70}", "");
    let mut rows: Vec<(&&str, &usize)> = totals.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    for (label, count) in rows {
        let pct = 100.0 * (*count as f64) / (samples_total as f64);
        println!("  {:>6.2}%  {}", pct, label);
    }

    Ok(())
}
