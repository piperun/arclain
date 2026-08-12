//! Static inspection of network policy stored by an older embedded Arclain.
//!
//! This path deliberately does not construct [`crate::ArclainApp`], start a
//! runtime, initialize plugins, ensure schemas, or create profile directories.

use std::collections::BTreeMap;
use std::path::Path;

use arclain_db::{LegacyInspectionError, LegacyInspectionErrorKind};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::runtime::{legacy_config_database, legacy_secrets_database};
use crate::settings::LegacyNetworkSettings;

fn inspection_error(error: LegacyInspectionError) -> ApplicationError {
    match error.kind() {
        LegacyInspectionErrorKind::Busy => ApplicationError::new(
            ApplicationErrorKind::Busy,
            "legacy network storage changed or is currently in use",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true),
        LegacyInspectionErrorKind::PermissionDenied => ApplicationError::new(
            ApplicationErrorKind::PermissionDenied,
            "legacy network storage cannot be read safely",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::CheckPermissions),
        LegacyInspectionErrorKind::Backend => ApplicationError::new(
            ApplicationErrorKind::Backend,
            "legacy network storage is invalid",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::Fatal),
    }
}

fn parse_plugin_proxy_settings(
    stored: Option<String>,
) -> Result<BTreeMap<String, bool>, ApplicationError> {
    match stored {
        None => Ok(BTreeMap::new()),
        Some(stored) => serde_json::from_str(&stored).map_err(|_| {
            ApplicationError::new(
                ApplicationErrorKind::Backend,
                "legacy plugin proxy settings are malformed",
            )
            .with_diagnostic("plugin_proxy_settings is not a boolean map")
            .with_recoverability(Recoverability::Fatal)
        }),
    }
}

/// Reads an existing legacy profile without creating or mutating any part of
/// it. Missing profile storage, table, or singleton row is absence, not an
/// error. Malformed existing storage fails closed as a bounded backend error.
pub fn inspect_legacy_network_settings(
    profile_data_dir: &Path,
) -> Result<Option<LegacyNetworkSettings>, ApplicationError> {
    let secrets_path = legacy_secrets_database(profile_data_dir);
    let secrets_lease = arclain_db::lock_and_inspect_legacy_socks5_password(&secrets_path)
        .map_err(inspection_error)?;
    let socks5_password_configured = secrets_lease
        .as_ref()
        .is_some_and(|lease| lease.socks5_password_configured());

    let config_result =
        arclain_db::inspect_legacy_network_row(&legacy_config_database(profile_data_dir));
    if let Some(lease) = secrets_lease {
        lease.finish().map_err(inspection_error)?;
    }
    let Some(row) = config_result.map_err(inspection_error)? else {
        return Ok(None);
    };

    Ok(Some(LegacyNetworkSettings {
        socks5_enabled: row.socks5_enabled,
        socks5_address: row.socks5_address,
        socks5_username: row.socks5_username,
        socks5_password_configured,
        plugin_proxy_enabled: parse_plugin_proxy_settings(row.plugin_proxy_settings)?,
    }))
}
