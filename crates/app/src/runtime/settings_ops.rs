//! `AppRuntime`-touching execution logic for the settings/secrets/vault
//! facade surface. `crate::settings` holds the DTOs and pure
//! validation/patch-application logic this module calls into;
//! `crate::runtime`'s own `impl ArclainApp` (its delimited "Task 10"
//! section) exposes the thin public methods that dispatch to the
//! functions here, exactly the way `runtime::processing_ops` backs
//! `start_convert`/`start_organize`/`start_pipeline`.
//!
//! ## Serializing mutations
//!
//! `SessionStore::mutable` is a `parking_lot::RwLock` -- fast, but not
//! async-aware, so nothing here holds it across an `.await`. Every
//! mutating operation instead takes `AppRuntime::settings_write_lock`
//! (a `tokio::sync::Mutex<()>`) for its *entire* read-validate-persist-
//! commit sequence, so at most one settings/vault mutation is ever
//! in flight at a time end to end: read a consistent snapshot, validate,
//! perform the real (possibly slow) I/O with no lock held over it, then
//! re-take the fast lock only to commit. This closes the race a bare
//! optimistic-revision check would otherwise leave open (a second
//! mutation committing between this call's own read and its own commit,
//! which no post-hoc revision re-check can undo once disk writes have
//! already happened) without needing full cross-resource rollback.
//! Read-only methods (`settings`, `password_rules`, `organization_profiles`)
//! never take this lock -- they only ever take the fast `RwLock` for the
//! instant it takes to clone out a snapshot.

use std::path::PathBuf;
use std::sync::Arc;

use arclain_core::services::SecretsService;
use arclain_core::DbPaths;

use crate::challenge::SecretInput;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::settings::{
    self, OrganizationProfileSummary, PasswordRuleInput, PasswordRuleSummary, SettingsPatch,
    SettingsSnapshot,
};

use super::AppRuntime;

const PROXY_PASSWORD_KEY: &str = "proxy:socks5";
const GAMETA_API_KEY_KEY: &str = "gameta:api_key";

// ============================================================================
// Read-only.
// ============================================================================

pub(super) async fn run_settings(
    inner: &Arc<AppRuntime>,
) -> Result<SettingsSnapshot, ApplicationError> {
    let mutable = inner.session.mutable.read();
    let socks5_password_configured = match mutable.dbs.as_ref() {
        Some(dbs) => secret_configured(dbs, PROXY_PASSWORD_KEY)?,
        None => false,
    };
    let gameta_api_key_configured = match mutable.dbs.as_ref() {
        Some(dbs) => secret_configured(dbs, GAMETA_API_KEY_KEY)?,
        None => false,
    };
    Ok(SettingsSnapshot {
        revision: mutable.revision,
        archive: settings::archive_dto(&mutable.user_config),
        network: settings::network_dto(
            &mutable.user_config,
            socks5_password_configured,
            gameta_api_key_configured,
        ),
        security: settings::security_dto(&mutable),
    })
}

pub(super) async fn run_organization_profiles(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    let config_db_path = {
        let mutable = inner.session.mutable.read();
        mutable
            .db_paths
            .as_ref()
            .map(|paths| paths.config_db.clone())
    };
    let Some(config_db_path) = config_db_path else {
        return Ok(Vec::new());
    };
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };
    let profiles = handle
        .spawn_blocking(move || {
            arclain_core::features::organization::list_archive_profiles(&config_db_path)
                .map_err(|error| backend_error("listing archive profiles", error))
        })
        .await
        .map_err(internal_join_error)??;
    Ok(profiles.iter().map(settings::summarize_profile).collect())
}

pub(super) async fn run_password_rules(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<PasswordRuleSummary>, ApplicationError> {
    let mutable = inner.session.mutable.read();
    Ok(mutable
        .pass_rules
        .iter()
        .map(settings::summarize_pass_rule)
        .collect())
}

// ============================================================================
// Mutations. Every one of these takes `settings_write_lock` for its whole
// duration -- see this module's own doc comment.
// ============================================================================

pub(super) async fn run_update_settings(
    inner: &Arc<AppRuntime>,
    patch: SettingsPatch,
) -> Result<SettingsSnapshot, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;

    // Phase 1: consistent snapshot + revision check + pure validation.
    // No I/O yet -- an invalid patch is rejected before anything is
    // persisted or before the vault is touched.
    let (mut proposed_user_config, mut proposed_crc_policy, current_db_paths, current_dbs) = {
        let mutable = inner.session.mutable.read();
        if patch.expected_revision != mutable.revision {
            return Err(conflict_error(mutable.revision));
        }
        (
            mutable.user_config.clone(),
            mutable.encrypted_crc_policy.clone(),
            mutable.db_paths.clone(),
            mutable.dbs.clone(),
        )
    };

    let touches_socks5_identity = patch
        .network
        .as_ref()
        .map(settings::network_patch_touches_socks5_identity)
        .unwrap_or(false);
    let touches_vault_paths = patch
        .security
        .as_ref()
        .map(settings::security_patch_touches_vault_paths)
        .unwrap_or(false);

    if let Some(archive_patch) = patch.archive {
        settings::apply_archive_patch(&mut proposed_user_config, archive_patch)?;
    }
    if let Some(network_patch) = patch.network {
        settings::apply_network_patch(&mut proposed_user_config, network_patch)?;
    }
    if let Some(ref security_patch) = patch.security {
        settings::apply_security_value_patch(&mut proposed_crc_policy, security_patch)?;
    }
    if touches_socks5_identity {
        validate_proxy_for_storage(&proposed_user_config)?;
    }

    // Phase 2: I/O. `inner.session.mutable` is not touched by any of this
    // -- a failure at any point here leaves it exactly as phase 1 read
    // it, so `settings()` keeps reporting the pre-patch values.
    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;

    if touches_socks5_identity {
        let Some(dbs) = current_dbs.clone() else {
            return Err(vault_unavailable_error());
        };
        let candidate = proposed_user_config.clone();
        let save_dbs = dbs.clone();
        let handle = inner
            .tokio_handle()
            .ok_or_else(shutdown_mid_request_error)?;
        handle
            .spawn_blocking(move || {
                let existing_password = save_dbs
                    .secrets
                    .get_secret(PROXY_PASSWORD_KEY)
                    .map_err(|error| backend_error("reading proxy password", error))?;
                let existing_password = existing_password.as_deref().map(|value| value.as_str());
                arclain_core::services::NetworkProxyPersistenceService::new(
                    &config_service,
                    &save_dbs.secrets,
                )
                .save(&candidate, existing_password)
                .map_err(|error| persistence_error("saving network settings", error))
                .map(|_outcome| ())
            })
            .await
            .map_err(internal_join_error)??;
        // Mirrors the pre-facade `SettingsAction::SaveNetwork` handler's
        // own final step: a saved proxy setting is inert until the live
        // `AsyncHttpClient` actually routes through it. Best-effort --
        // see the helper's own doc comment for why a failure here does
        // not fail the whole `update_settings` call.
        apply_live_proxy_routing(inner, &dbs, &proposed_user_config).await;
    } else {
        let candidate = proposed_user_config.clone();
        let handle = inner
            .tokio_handle()
            .ok_or_else(shutdown_mid_request_error)?;
        handle
            .spawn_blocking(move || {
                config_service
                    .save_user_config(&candidate)
                    .map_err(|error| persistence_error("saving settings", error))
            })
            .await
            .map_err(internal_join_error)??;
    }

    let vault_repoint = if touches_vault_paths {
        Some(
            repoint_vault_paths(
                inner,
                current_db_paths.clone(),
                patch
                    .security
                    .as_ref()
                    .expect("touches_vault_paths implies Some"),
            )
            .await?,
        )
    } else {
        None
    };

    persist_encrypted_crc_policy(inner, &current_db_paths, &proposed_crc_policy).await?;

    // Phase 3: commit. Re-checks the revision so a second mutation that
    // committed while this one was performing I/O (phase 2) is caught
    // here too, even though `settings_write_lock` makes that window
    // exactly zero for calls that go through this same facade -- the
    // check costs nothing and stays correct even if a future caller
    // someday bypasses the lock (e.g. a direct `mutable.write()` from
    // new code added without reading this module's doc comment).
    let mut mutable = inner.session.mutable.write();
    if patch.expected_revision != mutable.revision {
        return Err(conflict_error(mutable.revision));
    }
    mutable.user_config = proposed_user_config;
    mutable.encrypted_crc_policy = proposed_crc_policy;
    if let Some((db_paths, dbs, pass_rules)) = vault_repoint {
        mutable.db_paths = Some(db_paths);
        mutable.dbs = Some(dbs);
        mutable.pass_rules = pass_rules;
    }
    mutable.revision += 1;

    let socks5_password_configured = match mutable.dbs.as_ref() {
        Some(dbs) => secret_configured(dbs, PROXY_PASSWORD_KEY)?,
        None => false,
    };
    let gameta_api_key_configured = match mutable.dbs.as_ref() {
        Some(dbs) => secret_configured(dbs, GAMETA_API_KEY_KEY)?,
        None => false,
    };
    Ok(SettingsSnapshot {
        revision: mutable.revision,
        archive: settings::archive_dto(&mutable.user_config),
        network: settings::network_dto(
            &mutable.user_config,
            socks5_password_configured,
            gameta_api_key_configured,
        ),
        security: settings::security_dto(&mutable),
    })
}

pub(super) async fn run_set_gameta_api_key(
    inner: &Arc<AppRuntime>,
    value: SecretInput,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = {
        let mutable = inner.session.mutable.read();
        mutable.dbs.clone().ok_or_else(vault_unavailable_error)?
    };
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let secret_value = value.expose_secret().to_string();
    handle
        .spawn_blocking(move || {
            dbs.secrets
                .set_secret(GAMETA_API_KEY_KEY, &secret_value)
                .map_err(|error| persistence_error("saving gameta API key", error))
        })
        .await
        .map_err(internal_join_error)??;
    bump_revision(inner);
    Ok(())
}

pub(super) async fn run_set_socks5_password(
    inner: &Arc<AppRuntime>,
    value: Option<SecretInput>,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = {
        let mutable = inner.session.mutable.read();
        mutable.dbs.clone().ok_or_else(vault_unavailable_error)?
    };
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let secret_value = value
        .as_ref()
        .map(|value| value.expose_secret().to_string());
    handle
        .spawn_blocking(move || match secret_value {
            Some(password) => dbs
                .secrets
                .set_secret(PROXY_PASSWORD_KEY, &password)
                .map_err(|error| persistence_error("saving SOCKS5 password", error)),
            None => dbs
                .secrets
                .remove_secret(PROXY_PASSWORD_KEY)
                .map_err(|error| persistence_error("clearing SOCKS5 password", error)),
        })
        .await
        .map_err(internal_join_error)??;
    bump_revision(inner);
    Ok(())
}

pub(super) async fn run_move_vault(
    inner: &Arc<AppRuntime>,
    destination: PathBuf,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let current_db_paths = {
        let mut mutable = inner.session.mutable.write();
        // Close the vault handle this instance currently holds before
        // touching its file -- the OS (Windows especially) refuses to
        // copy a file this same process still has open. Mirrors the
        // pre-facade `AppState::move_vault`'s own `self.dbs.take()` step
        // exactly, including that step's own limitation: a failure from
        // here on leaves the vault unavailable until a later successful
        // move/rekey (or restart) reopens it, the same as it always has.
        mutable.dbs = None;
        mutable.db_paths.clone()
    };
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let destination_display = destination.clone();
    let (new_paths, new_dbs) = handle
        .spawn_blocking(move || {
            let mut db_paths = current_db_paths;
            SecretsService::move_vault(&mut db_paths, &destination_display.to_string_lossy())
                .map_err(|error| persistence_error("moving the vault", error))
        })
        .await
        .map_err(internal_join_error)??;
    let pass_rules = load_pass_rules(&new_dbs)?;

    let mut mutable = inner.session.mutable.write();
    mutable.db_paths = Some(new_paths);
    mutable.dbs = Some(new_dbs);
    mutable.pass_rules = pass_rules;
    mutable.revision += 1;
    Ok(())
}

pub(super) async fn run_rekey_vault(
    inner: &Arc<AppRuntime>,
    key_file: PathBuf,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let current_db_paths = {
        let mut mutable = inner.session.mutable.write();
        // See `run_move_vault`'s identical step -- `SecretsService::
        // rekey_vault` additionally *deletes* the old vault file, which
        // fails outright on Windows while this process still has it
        // open, not just the copy `move_vault` performs.
        mutable.dbs = None;
        mutable.db_paths.clone()
    };
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let key_file_display = key_file.clone();
    let (new_paths, new_dbs, db_pass_rules) = handle
        .spawn_blocking(move || {
            let mut db_paths = current_db_paths;
            SecretsService::rekey_vault(&mut db_paths, &key_file_display.to_string_lossy())
                .map_err(|error| persistence_error("rekeying the vault", error))
        })
        .await
        .map_err(internal_join_error)??;

    let mut mutable = inner.session.mutable.write();
    mutable.db_paths = Some(new_paths);
    mutable.dbs = Some(new_dbs);
    mutable.pass_rules = db_pass_rules.into_iter().map(to_core_pass_rule).collect();
    mutable.revision += 1;
    Ok(())
}

pub(super) async fn run_upsert_password_rule(
    inner: &Arc<AppRuntime>,
    rule: PasswordRuleInput,
) -> Result<Vec<PasswordRuleSummary>, ApplicationError> {
    settings::validate_password_rule_input(&rule)?;
    let _write_guard = inner.settings_write_lock.lock().await;

    let (dbs, mut rules) = {
        let mutable = inner.session.mutable.read();
        let dbs = mutable.dbs.clone().ok_or_else(vault_unavailable_error)?;
        (dbs, mutable.pass_rules.clone())
    };

    let existing_index = rules.iter().position(|existing| existing.name == rule.name);
    let password = match rule.password {
        Some(secret) => secret.expose_secret().to_string(),
        None => {
            let Some(index) = existing_index else {
                return Err(password_required_for_new_rule_error());
            };
            rules[index].password.clone()
        }
    };
    let new_rule = arclain_core::PassRule {
        name: rule.name,
        pattern: rule.pattern,
        password,
        priority: rule.priority,
        enabled: rule.enabled,
    };
    match existing_index {
        Some(index) => rules[index] = new_rule,
        None => rules.push(new_rule),
    }

    persist_pass_rules(inner, &dbs, rules.clone()).await?;

    let mut mutable = inner.session.mutable.write();
    mutable.pass_rules = rules;
    Ok(mutable
        .pass_rules
        .iter()
        .map(settings::summarize_pass_rule)
        .collect())
}

pub(super) async fn run_delete_password_rule(
    inner: &Arc<AppRuntime>,
    name: String,
) -> Result<Vec<PasswordRuleSummary>, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;

    let (dbs, mut rules) = {
        let mutable = inner.session.mutable.read();
        let dbs = mutable.dbs.clone().ok_or_else(vault_unavailable_error)?;
        (dbs, mutable.pass_rules.clone())
    };

    let Some(index) = rules.iter().position(|existing| existing.name == name) else {
        return Err(rule_not_found_error(&name));
    };
    rules.remove(index);

    persist_pass_rules(inner, &dbs, rules.clone()).await?;

    let mut mutable = inner.session.mutable.write();
    mutable.pass_rules = rules;
    Ok(mutable
        .pass_rules
        .iter()
        .map(settings::summarize_pass_rule)
        .collect())
}

// ============================================================================
// Shared helpers.
// ============================================================================

/// Re-applies SOCKS5 proxy routing (and the per-plugin proxy map) to this
/// instance's `AsyncHttpClient` after a settings save that touched a
/// SOCKS5 identity field -- mirrors the pre-facade `SettingsAction::
/// SaveNetwork` handler's own final step
/// (`shared.services.async_http_client.apply_proxy_routing(...)`), which
/// this replaces: `core_services().async_http_client` is the exact same
/// `Arc<AsyncHttpClient>` `crates/ui`'s `Services.async_http_client`
/// holds (see `crate::runtime::session_store`'s own doc comment on why
/// that Arc is safely shared), so applying it here is equally visible to
/// both.
///
/// Best-effort: a failure here is logged, not propagated as an
/// `update_settings` error. The setting itself already saved
/// successfully and was validated up front by
/// `validate_proxy_for_storage`; a routing-application hiccup on top of
/// that is a lesser, retryable concern, matching how `apply_proxy_to_client`
/// itself degrades (marks routing unavailable rather than treating it as
/// fatal) everywhere else it's called.
async fn apply_live_proxy_routing(
    inner: &Arc<AppRuntime>,
    dbs: &arclain_core::ConfigDbs,
    user_config: &arclain_core::UserConfig,
) {
    let async_http_client = inner.core_services().async_http_client.clone();
    let dbs = dbs.clone();
    let user_config = user_config.clone();
    let Some(handle) = inner.tokio_handle() else {
        return;
    };
    let result = handle
        .spawn_blocking(move || {
            let resolved =
                arclain_core::utilities::proxy::resolve_proxy_config(&user_config, &dbs.secrets)?;
            arclain_core::utilities::proxy::apply_proxy_to_client(
                &async_http_client,
                resolved,
                &user_config,
            )
        })
        .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(
                "settings: failed to apply live proxy routing after a settings save: {error:#}"
            );
        }
        Err(join_error) => {
            tracing::warn!(
                "settings: proxy routing task failed after a settings save: {join_error}"
            );
        }
    }
}

fn to_core_pass_rule(rule: arclain_core::DbPassRule) -> arclain_core::PassRule {
    arclain_core::PassRule {
        name: rule.name,
        pattern: rule.pattern,
        password: rule.password,
        priority: rule.priority,
        enabled: rule.enabled,
    }
}

fn load_pass_rules(
    dbs: &arclain_core::ConfigDbs,
) -> Result<Vec<arclain_core::PassRule>, ApplicationError> {
    dbs.secrets
        .list_pass_rules()
        .map(|rules| rules.into_iter().map(to_core_pass_rule).collect())
        .map_err(|error| persistence_error("loading password rules", error))
}

async fn persist_pass_rules(
    inner: &Arc<AppRuntime>,
    dbs: &arclain_core::ConfigDbs,
    rules: Vec<arclain_core::PassRule>,
) -> Result<(), ApplicationError> {
    let dbs = dbs.clone();
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    handle
        .spawn_blocking(move || {
            let db_rules: Vec<arclain_core::DbPassRule> = rules
                .into_iter()
                .map(|rule| arclain_core::DbPassRule {
                    name: rule.name,
                    pattern: rule.pattern,
                    password: rule.password,
                    priority: rule.priority,
                    enabled: rule.enabled,
                })
                .collect();
            dbs.secrets
                .replace_all_pass_rules(&db_rules)
                .map_err(|error| persistence_error("saving password rules", error))
        })
        .await
        .map_err(internal_join_error)?
}

async fn persist_encrypted_crc_policy(
    inner: &Arc<AppRuntime>,
    db_paths: &Option<DbPaths>,
    encrypted_crc_policy: &str,
) -> Result<(), ApplicationError> {
    let Some(db_paths) = db_paths.clone() else {
        return Ok(());
    };
    let policy = encrypted_crc_policy.to_string();
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    handle
        .spawn_blocking(move || {
            let cfg_conn = arclain_core::ConfigDb::open(&db_paths.config_db)
                .map_err(|error| persistence_error("opening config database", error))?
                .into_sqlite_db();
            cfg_conn
                .with_connection(|conn| {
                    arclain_core::set_config(conn, "encrypted_crc_policy", &policy)
                })
                .map_err(|error| persistence_error("saving CRC policy", error))
        })
        .await
        .map_err(internal_join_error)?
}

/// Persists a `secrets_database_path`/`key_file_path` change (see
/// `crate::settings::security_patch_touches_vault_paths`) and re-opens
/// the vault at the resulting paths, mirroring the pre-facade UI's own
/// `AppState::apply_preferences`. Unlike [`run_move_vault`]/
/// [`run_rekey_vault`], this *repoints* to a path the caller asserts
/// already holds a valid vault (or accepts creating a fresh one there);
/// it never copies or re-encrypts the current vault's contents.
async fn repoint_vault_paths(
    inner: &Arc<AppRuntime>,
    current_db_paths: Option<DbPaths>,
    patch: &crate::settings::SecuritySettingsPatch,
) -> Result<
    (
        arclain_core::DbPaths,
        arclain_core::ConfigDbs,
        Vec<arclain_core::PassRule>,
    ),
    ApplicationError,
> {
    let secrets_database_path = patch.secrets_database_path.clone();
    let key_file_path = patch.key_file_path.clone();
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    handle
        .spawn_blocking(move || {
            let mut paths = current_db_paths.unwrap_or(
                arclain_core::DbPaths::calculate_defaults("arclain").map_err(|error| {
                    persistence_error("resolving default database paths", error)
                })?,
            );
            let defaults = arclain_core::DbPaths::calculate_defaults("arclain")
                .map_err(|error| persistence_error("resolving default database paths", error))?;

            match secrets_database_path {
                crate::settings::PatchValue::Set(path) => paths.secrets_db = path,
                crate::settings::PatchValue::Clear => {
                    paths.secrets_db = defaults.secrets_db.clone()
                }
                crate::settings::PatchValue::Keep => {}
            }
            match key_file_path {
                crate::settings::PatchValue::Set(path) => paths.key_file = Some(path),
                crate::settings::PatchValue::Clear => paths.key_file = defaults.key_file.clone(),
                crate::settings::PatchValue::Keep => {}
            }

            // Persist the overrides before attempting to re-open, matching
            // `apply_preferences`'s own ordering: a crash between here and
            // the reopen below still leaves the *next* launch pointed at
            // the new location, rather than silently reverting.
            let cfg_conn = arclain_core::ConfigDb::open(&paths.config_db)
                .map_err(|error| persistence_error("opening config database", error))?
                .into_sqlite_db();
            cfg_conn
                .with_connection(|conn| {
                    arclain_core::set_config(
                        conn,
                        "secrets_db_path",
                        &paths.secrets_db.to_string_lossy(),
                    )?;
                    if let Some(key_file) = &paths.key_file {
                        arclain_core::set_config(
                            conn,
                            "key_file_path",
                            &key_file.to_string_lossy(),
                        )?;
                    }
                    Ok(())
                })
                .map_err(|error| persistence_error("saving vault path overrides", error))?;

            let Some(key_file) = paths.key_file.clone() else {
                return Err(vault_key_file_missing_error());
            };
            let key = arclain_core::SecretsKey::load_from_file(&key_file)
                .map_err(|error| persistence_error("loading the vault key file", error))?;
            let dbs = arclain_core::open_databases(&paths, &key).map_err(|error| {
                persistence_error("opening the vault at the new location", error)
            })?;
            let pass_rules = load_pass_rules(&dbs)?;
            Ok((paths, dbs, pass_rules))
        })
        .await
        .map_err(internal_join_error)?
}

fn secret_configured(dbs: &arclain_core::ConfigDbs, key: &str) -> Result<bool, ApplicationError> {
    dbs.secrets
        .get_secret(key)
        .map(|value| value.is_some())
        .map_err(|error| persistence_error("reading secret storage", error))
}

fn validate_proxy_for_storage(
    user_config: &arclain_core::UserConfig,
) -> Result<(), ApplicationError> {
    let config = arclain_network::features::proxy::ProxyConfig {
        enabled: user_config.socks5_enabled,
        address: user_config.socks5_address.clone().unwrap_or_default(),
        username: user_config.socks5_username.clone(),
        password: None,
    };
    config.validate_for_storage().map_err(|message| {
        ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "invalid SOCKS5 proxy settings",
        )
        .with_diagnostic(message)
        .with_recoverability(Recoverability::UserAction)
        .with_field("network.socks5_address")
    })
}

fn bump_revision(inner: &Arc<AppRuntime>) {
    inner.session.mutable.write().revision += 1;
}

fn backend_error(context: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Backend,
        "settings backend operation failed",
    )
    .with_diagnostic(format!("{context}: {error}"))
    .with_recoverability(Recoverability::Retry)
}

fn persistence_error(context: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Persistence,
        "failed to persist settings",
    )
    .with_diagnostic(format!("{context}: {error}"))
    .with_recoverability(Recoverability::Retry)
}

fn conflict_error(current_revision: u64) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "settings changed since expected_revision was read",
    )
    .with_diagnostic(format!("current revision is {current_revision}"))
    .with_recoverability(Recoverability::Retry)
    .with_suggested_action(SuggestedAction::Retry)
    .with_field("expected_revision")
}

fn settings_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "settings storage is unavailable: no configuration database is configured",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn vault_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "the encrypted vault is unavailable",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::SupplyPassword)
}

fn vault_key_file_missing_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "no vault key file is configured",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_field("security.key_file_path")
}

fn password_required_for_new_rule_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "a new password rule requires a password",
    )
    .with_diagnostic("password was None and no existing rule with this name has one to keep")
    .with_recoverability(Recoverability::UserAction)
    .with_field("password")
}

fn rule_not_found_error(name: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such password rule")
        .with_diagnostic(format!("rule {name:?} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("name")
}

fn shutdown_mid_request_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "application is shutting down",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
        .with_diagnostic(join_error.to_string())
}
