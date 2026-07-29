//! `arclain-cli settings show [--json]` / `settings set-sevenzip-path PATH` /
//! `settings set-backend-mode MODE`

use std::path::PathBuf;

use arclain_app::settings::{ArchiveSettingsPatch, BackendModeDto, PatchValue, SettingsSnapshot};
use arclain_app::ArclainApp;
use clap::{Args, Subcommand, ValueEnum};

use crate::output::{exit_code, exit_code_for, print_error, print_json};

#[derive(Debug, Subcommand)]
pub enum SettingsCommand {
    /// Show every non-secret application setting. Secret fields
    /// (vault passwords, the gameta API key, ...) are never part of
    /// this output at all -- the DTOs `ArclainApp::settings` returns
    /// only ever carry a `*_configured: bool` flag for those, never the
    /// value itself.
    Show,
    /// Sets the 7-Zip executable path.
    SetSevenzipPath(SevenzipPathArgs),
    /// Sets the archive backend mode ("cli" or "native").
    SetBackendMode(BackendModeArgs),
}

#[derive(Debug, Args)]
pub struct SevenzipPathArgs {
    pub path: PathBuf,
}

/// Mirrors `arclain_app::settings::BackendModeDto`'s own `snake_case`
/// spelling.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum BackendModeArg {
    Cli,
    Native,
}

impl BackendModeArg {
    fn to_facade(self) -> BackendModeDto {
        match self {
            Self::Cli => BackendModeDto::Cli,
            Self::Native => BackendModeDto::Native,
        }
    }
}

#[derive(Debug, Args)]
pub struct BackendModeArgs {
    #[arg(value_enum)]
    pub mode: BackendModeArg,
}

pub async fn dispatch(app: &ArclainApp, command: &SettingsCommand, json: bool) -> i32 {
    match command {
        SettingsCommand::Show => run_show(app, json).await,
        SettingsCommand::SetSevenzipPath(args) => {
            let path = args.path.clone();
            run_set_archive_field(app, json, move |patch| {
                patch.sevenzip_path = PatchValue::Set(path.clone());
            })
            .await
        }
        SettingsCommand::SetBackendMode(args) => {
            let mode = args.mode;
            run_set_archive_field(app, json, move |patch| {
                patch.backend_mode = PatchValue::Set(mode.to_facade());
            })
            .await
        }
    }
}

async fn run_show(app: &ArclainApp, json: bool) -> i32 {
    match app.settings().await {
        Ok(snapshot) => {
            if json {
                print_json(&snapshot);
            } else {
                print_settings_human(&snapshot);
            }
            exit_code::SUCCESS
        }
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            code
        }
    }
}

fn print_settings_human(snapshot: &SettingsSnapshot) {
    println!("revision: {}", snapshot.revision);
    println!("archive.backend_mode: {:?}", snapshot.archive.backend_mode);
    match &snapshot.archive.sevenzip_path {
        Some(path) => println!("archive.sevenzip_path: {}", path.display()),
        None => println!("archive.sevenzip_path: (system default)"),
    }
    println!(
        "network.socks5_enabled: {}",
        snapshot.network.socks5_enabled
    );
    println!(
        "network.gameta_server_enabled: {}",
        snapshot.network.gameta_server_enabled
    );
    println!(
        "security.vault_available: {}",
        snapshot.security.vault_available
    );
    println!(
        "security.encrypted_crc_policy: {}",
        snapshot.security.encrypted_crc_policy
    );
}

/// Reads a fresh `SettingsSnapshot` to learn the current `revision`,
/// applies `mutate` to an all-`Keep` `ArchiveSettingsPatch`, and submits
/// it as a `SettingsPatch` touching only the `archive` section -- shared
/// by every `settings set-*` command this task adds (`set-sevenzip-path`,
/// `set-backend-mode`).
async fn run_set_archive_field(
    app: &ArclainApp,
    json: bool,
    mutate: impl FnOnce(&mut ArchiveSettingsPatch),
) -> i32 {
    let snapshot = match app.settings().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    let mut patch = ArchiveSettingsPatch {
        backend_mode: PatchValue::Keep,
        cache_directory: PatchValue::Keep,
        temp_directory: PatchValue::Keep,
        transfer_directory: PatchValue::Keep,
        sevenzip_path: PatchValue::Keep,
    };
    mutate(&mut patch);

    let request = arclain_app::settings::SettingsPatch {
        expected_revision: snapshot.revision,
        archive: Some(patch),
        network: None,
        security: None,
    };

    match app.update_settings(request).await {
        Ok(updated) => {
            if json {
                print_json(&updated);
            } else {
                println!("settings updated to revision {}", updated.revision);
            }
            exit_code::SUCCESS
        }
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            code
        }
    }
}
