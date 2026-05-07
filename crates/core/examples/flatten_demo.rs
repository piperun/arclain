//! Manual verification tool for the flatten pipeline.
//!
//! Runs `flatten_nested_archives_recursive` against a directory using
//! the 7-Zip CLI backend and prints the resulting folder names. Used
//! to spot-check real mod data after pipeline refactors — not part of
//! the test suite because it depends on the user having 7z on PATH
//! and a directory containing actual archives.
//!
//! Run with:
//!
//!     cargo run -p arclain_core --example flatten_demo -- "<dir>"
//!
//! The directory must contain pre-extracted archives (the outer
//! container has already been opened so inner archive files sit at
//! the top level). Flatten then extracts each inner archive,
//! optionally strips a common prefix from the resulting folder names,
//! and renames each folder to its `modinfo.ini` `name=` value if
//! present.
//!
//! ⚠️ This MUTATES the directory: archives are extracted to sibling
//! folders and the original archive files are deleted. Run on a copy
//! of your data, never on the canonical RAW dir.

use anyhow::Result;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::features::conversion::flatten::flatten_nested_archives_recursive;
use arclain_core::ArchiveBackend;
use std::env;
use std::path::PathBuf;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <directory> [--strip-prefix]", args[0]);
        eprintln!();
        eprintln!("Runs the flatten pipeline on a directory. WARNING: mutates the");
        eprintln!("directory in place — extracts archives and deletes originals.");
        std::process::exit(1);
    }

    let dir = PathBuf::from(&args[1]);
    let strip_prefix = args.iter().any(|a| a == "--strip-prefix");

    let backend = SevenZipCli::detect(None)?;

    println!("Running flatten on: {}", dir.display());
    println!("strip_prefix: {}", strip_prefix);
    println!();

    let report = flatten_nested_archives_recursive(
        &dir,
        strip_prefix,
        0, // unlimited depth (still bounded by FLATTEN_MAX_ITERATIONS internally)
        |archive, dest| backend.extract_all(archive, dest, None),
    )?;

    println!("Extracted ({}):", report.extracted.len());
    for name in &report.extracted {
        println!("  {}", name);
    }

    if !report.skipped.is_empty() {
        println!();
        println!("Skipped ({}):", report.skipped.len());
        for name in &report.skipped {
            println!("  {}", name);
        }
    }

    if !report.failed.is_empty() {
        println!();
        println!("Failed ({}):", report.failed.len());
        for (name, err) in &report.failed {
            println!("  {}: {}", name, err);
        }
    }

    Ok(())
}
