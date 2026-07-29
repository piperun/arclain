//! `arclain-cli`: a pure `arclain_app::ArclainApp` client exposing a
//! read-only surface (`inspect`/`list`/`profiles`) over Arclain's archive
//! and settings facade.
//!
//! Architecture: this binary depends on `arclain_app` only -- it may
//! format the DTOs the facade returns, but it never reaches into
//! `arclain_core`/`arclain_plugins`/etc directly (see
//! `scripts/frontend_boundary.py`'s `FRONTEND_CRATES`). `crate::commands`
//! holds the argument surface and per-command logic; `crate::output`
//! holds the shared JSON envelope, exit-code mapping, and printing
//! helpers every command uses.
//!
//! # Runtime shape
//!
//! `ArclainApp::bootstrap` is synchronous and builds its own internal
//! Tokio runtime (see `arclain_app::runtime`'s own doc comment); this
//! binary calls it directly from a plain, non-async `main`, before any
//! ambient runtime exists at all. Only afterwards does it build a small
//! `current_thread` runtime of its own -- entirely separate from the
//! application's internal one -- to await the facade's async methods and
//! read its operation-event stream, matching the documented pattern for
//! an async consumer that is not itself a frontend embedded in a foreign
//! async runtime (a Flutter bridge, egui's async integration): bootstrap
//! sync first, then a small runtime purely for this process's own awaits.

mod commands;
mod output;

use clap::{CommandFactory, FromArgMatches};

use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use commands::Cli;
use output::exit_code;

fn main() {
    let code = run();
    std::process::exit(code);
}

/// Parses arguments with color output disabled unconditionally (belt and
/// suspenders alongside clap's own non-tty auto-detection): this CLI's
/// `--json` output must never contain an ANSI escape sequence, and
/// disabling color at the `Command` level covers `--help`/usage-error
/// text too, not just this crate's own `println!`/`eprintln!` calls.
///
/// A parse error or `--help`/`--version` request exits the process from
/// inside `error.exit()` (clap's own convention: `0` for help/version,
/// `2` for a real usage error) before this function returns at all.
fn parse_args() -> Cli {
    let command = Cli::command().color(clap::ColorChoice::Never);
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
}

/// Builds the paths this invocation bootstraps against: `--config-dir`'s
/// subdirectories if given, otherwise `None` (system defaults).
fn resolve_paths_override(config_dir: Option<&std::path::Path>) -> Option<AppPaths> {
    let root = config_dir?;
    Some(AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
    })
}

fn run() -> i32 {
    let cli = parse_args();

    let mut config = BootstrapConfig::system_default();
    config.paths_override = resolve_paths_override(cli.config_dir.as_deref());

    let app = match ArclainApp::bootstrap(config) {
        Ok(app) => app,
        Err(error) => {
            let code = output::exit_code_for(&error.kind);
            output::print_error(&error);
            return code;
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            output::print_plain_error(&format!("failed to start the CLI's runtime: {error}"));
            return exit_code::INTERNAL_FAILURE;
        }
    };

    runtime.block_on(async {
        let code = commands::dispatch(&app, &cli.command, cli.json).await;
        // Best-effort: an already-successful command should not be
        // reported as failed just because shutdown itself hiccups.
        let _ = app.shutdown().await;
        code
    })
}
