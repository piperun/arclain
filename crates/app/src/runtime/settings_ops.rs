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
//! Read-only methods (`settings`, `password_rules`) never take this lock
//! -- they only ever take the fast `RwLock` for the instant it takes to
//! clone out a snapshot. `runtime::organization_ops` (archive profiles,
//! organization rules) takes this same lock for *its* mutations, so a
//! profile save and a vault move can never interleave.
//!
//! ## The precise atomicity guarantee `update_settings` makes (and does not)
//!
//! `run_update_settings` performs up to **three independent disk writes**
//! in sequence, none wrapped in one cross-write transaction (SQLite and
//! redb are separate engines with no shared transaction boundary, and
//! `repoint_vault_paths` additionally opens a whole new database):
//!
//! 1. The `user_config` row (archive/network fields), via either
//!    `ConfigService::save_user_config` or, when the patch touches a
//!    SOCKS5 identity field, the journaled `NetworkProxyPersistenceService::
//!    save`.
//! 2. `repoint_vault_paths`, only when the security patch touches
//!    `secrets_database_path`/`key_file_path`: persists the path override
//!    then re-opens the vault at the new location.
//! 3. `persist_app_config_policies`: one `app_config` key/value write
//!    covering both the encrypted-CRC policy and the pipeline collision
//!    default (see that function's own doc comment for why it writes
//!    both every time).
//!
//! **What is guaranteed**: if step 1 fails, nothing changes -- the
//! in-memory `mutable` state (and therefore every subsequent `settings()`
//! call) is untouched, because it is only written in the final commit
//! phase, after every write above has already succeeded. This is proven
//! by `a_forced_write_failure_leaves_settings_completely_unchanged` and
//! `set_socks5_password_fails_cleanly_instead_of_ignoring_a_corrupt_pending_marker`.
//!
//! **What is *not* guaranteed**: if step 1 succeeds but step 2 or step 3
//! fails, disk and this instance's in-memory cache can diverge for as
//! long as this instance keeps running -- step 1's write already landed
//! on disk, but the in-memory commit (phase 3) never runs, so `settings()`
//! keeps reporting the pre-patch values until a *later* successful
//! `update_settings` call catches the cache up (its own step 1 re-reads
//! the current on-disk row -- see the "C1" doc note above -- which
//! picks up whatever step 1 previously wrote) or the process restarts
//! (which re-reads everything from disk fresh). This divergence is
//! narrow (steps 2/3 only run for security-patch fields, a small
//! fraction of calls) and matches this codebase's existing "propagate
//! the first failure, don't silently swallow it" standard (the H4 audit
//! fix) rather than full cross-engine ACID -- reordering to make the
//! in-memory commit strictly last-and-only-on-total-success would still
//! leave the *disk* itself inconsistent across engines/files in the same
//! way, so it would trade one honestly-documented gap for a differently
//! shaped one, not remove it. `repoint_vault_paths_failure_leaves_
//! settings_unchanged_in_memory` and
//! `persist_encrypted_crc_policy_failure_leaves_settings_unchanged_in_memory`
//! (both in `tests/settings_facade.rs`) pin down exactly this documented
//! end state for steps 2 and 3 respectively.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use arclain_core::services::SecretsService;
use arclain_core::DbPaths;
use arclain_network::features::proxy::ConnectionTestResult;

use crate::challenge::SecretInput;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::plugins::PluginSettingsSnapshot;
use crate::settings::{
    self, CacheMaintenanceReport, CacheMaintenanceTask, GametaServerInfo, NetworkProbeReport,
    PasswordRuleEditInput, PasswordRuleInput, PasswordRuleSummary, ProbeStepDto, SettingsPatch,
    SettingsSnapshot, Socks5Candidate,
};

use super::AppRuntime;

const PROXY_PASSWORD_KEY: &str = "proxy:socks5";
const GAMETA_API_KEY_KEY: &str = "gameta:api_key";

pub(super) fn run_cache_maintenance(
    inner: &AppRuntime,
    task: CacheMaintenanceTask,
) -> Result<CacheMaintenanceReport, ApplicationError> {
    let _guard = inner.cache_maintenance_lock.lock();
    match task {
        CacheMaintenanceTask::ClearContent => {
            let cache = inner.content_cache().ok_or_else(cache_unavailable_error)?;
            cache
                .clear_content()
                .map_err(|error| cache_maintenance_error("clearing cache content", error))?;
            Ok(CacheMaintenanceReport::ContentCleared)
        }
        task => {
            let dbs = inner
                .session
                .mutable
                .read()
                .dbs
                .clone()
                .ok_or_else(cache_unavailable_error)?;
            match task {
                CacheMaintenanceTask::ClearIndex => {
                    dbs.metadata.clear_cache_index().map_err(|error| {
                        cache_maintenance_error("clearing the cache index", error)
                    })?;
                    Ok(CacheMaintenanceReport::IndexCleared)
                }
                CacheMaintenanceTask::GarbageCollect => {
                    let entries =
                        dbs.metadata
                            .delete_orphaned_cache_entries()
                            .map_err(|error| {
                                cache_maintenance_error("removing orphaned cache entries", error)
                            })?;
                    Ok(CacheMaintenanceReport::OrphansRemoved { entries })
                }
                CacheMaintenanceTask::CleanOldSearch => {
                    let entries = dbs.metadata.delete_old_search_cache(7).map_err(|error| {
                        cache_maintenance_error("removing old search-cache entries", error)
                    })?;
                    Ok(CacheMaintenanceReport::OldSearchEntriesRemoved { entries })
                }
                CacheMaintenanceTask::RepairEntries => {
                    let (cache_types, product_ids) =
                        dbs.metadata.migrate_fix_cache_entries().map_err(|error| {
                            cache_maintenance_error("repairing cache entries", error)
                        })?;
                    Ok(CacheMaintenanceReport::EntriesRepaired {
                        cache_types,
                        product_ids,
                    })
                }
                CacheMaintenanceTask::ClearContent => unreachable!("handled above"),
            }
        }
    }
}

fn cache_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "cache storage is unavailable",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn cache_maintenance_error(context: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Persistence,
        "cache maintenance failed",
    )
    .with_diagnostic(format!("{context}: {error}"))
    .with_recoverability(Recoverability::Retry)
    .with_retryable(true)
    .with_suggested_action(SuggestedAction::Retry)
}

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
        archive: settings::archive_dto(&mutable.user_config, &mutable.default_collision_policy),
        network: settings::network_dto(
            &mutable.user_config,
            socks5_password_configured,
            gameta_api_key_configured,
        ),
        security: settings::security_dto(&mutable),
        general: settings::general_dto(&mutable.user_config),
    })
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

    // Phase 1: consistent snapshot + revision check + validation. The
    // only I/O here is a read-only secrets lookup for the
    // currently-stored SOCKS5 password (only when the patch touches a
    // SOCKS5 identity field) -- needed so `validate_proxy_for_storage`
    // checks the exact username+password combination that will actually
    // be in effect afterward (see that function's own doc comment).
    // Nothing is ever written, and the vault is never re-opened, until
    // every check below has passed.
    let (
        current_crc_policy,
        current_collision_policy,
        current_db_paths,
        default_db_paths,
        current_dbs,
    ) = {
        let mutable = inner.session.mutable.read();
        if patch.expected_revision != mutable.revision {
            return Err(conflict_error(mutable.revision));
        }
        (
            mutable.encrypted_crc_policy.clone(),
            mutable.default_collision_policy.clone(),
            mutable.db_paths.clone(),
            mutable.default_db_paths.clone(),
            mutable.dbs.clone(),
        )
    };
    let mut proposed_crc_policy = current_crc_policy;
    let mut proposed_collision_policy = current_collision_policy;

    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;

    // Re-read the *current on-disk* `user_config` row rather than
    // trusting this instance's cached `mutable.user_config` copy.
    // `UserConfig` is one row with 22+ columns; patching a stale cached
    // copy and writing the whole row back would silently revert every
    // column a concurrent writer touched since this instance last saw
    // them. Every egui-side writer now goes through this facade (either
    // this patch, or `set_plugin_enabled`/`set_plugin_settings`'s own
    // dedicated small writes -- see their doc comments for why those stay
    // separate from this generic patch), so the only remaining window is
    // a genuinely concurrent call racing strictly between this read and
    // this call's own write below, with no shared lock protecting it
    // across process boundaries -- not something a read-then-write
    // ordering on this side can fully close by itself.
    let mut proposed_user_config = {
        let read_config_service = config_service.clone();
        let handle = inner
            .tokio_handle()
            .ok_or_else(shutdown_mid_request_error)?;
        handle
            .spawn_blocking(move || {
                read_config_service
                    .get_user_config()
                    .map_err(|error| backend_error("reading current settings", error))
            })
            .await
            .map_err(internal_join_error)??
    };

    let touches_socks5_identity = patch
        .network
        .as_ref()
        .map(settings::network_patch_touches_socks5_identity)
        .unwrap_or(false);
    let touches_plugin_proxy_map = patch
        .network
        .as_ref()
        .map(settings::network_patch_touches_plugin_proxy_map)
        .unwrap_or(false);
    let touches_vault_paths = patch
        .security
        .as_ref()
        .map(settings::security_patch_touches_vault_paths)
        .unwrap_or(false);

    // Read-only, and only when needed: the SOCKS5 identity fields are
    // the only ones whose validation depends on a secret this patch
    // itself never carries (see `validate_proxy_for_storage`'s doc
    // comment). Read once here and reused by Phase 2 below instead of
    // read again there -- `settings_write_lock` is held for this whole
    // call, so nothing can change the stored password out from under
    // either use.
    let existing_socks5_password = if touches_socks5_identity {
        match current_dbs.clone() {
            Some(dbs) => {
                let handle = inner
                    .tokio_handle()
                    .ok_or_else(shutdown_mid_request_error)?;
                handle
                    .spawn_blocking(move || {
                        dbs.secrets
                            .get_secret(PROXY_PASSWORD_KEY)
                            .map_err(|error| backend_error("reading proxy password", error))
                    })
                    .await
                    .map_err(internal_join_error)??
            }
            // No vault yet: Phase 2 below fails this call with
            // `vault_unavailable_error` regardless, exactly as before
            // this fix -- validating with no password here changes
            // nothing about that outcome.
            None => None,
        }
    } else {
        None
    };

    if let Some(archive_patch) = patch.archive {
        settings::apply_archive_patch(
            &mut proposed_user_config,
            &mut proposed_collision_policy,
            archive_patch,
        )?;
    }
    if let Some(network_patch) = patch.network {
        settings::apply_network_patch(&mut proposed_user_config, network_patch)?;
    }
    if let Some(general_patch) = patch.general {
        settings::apply_general_patch(&mut proposed_user_config, general_patch)?;
    }
    if let Some(ref security_patch) = patch.security {
        settings::apply_security_value_patch(&mut proposed_crc_policy, security_patch)?;
    }
    if touches_socks5_identity {
        let validation_password = existing_socks5_password
            .as_deref()
            .map(|value| value.as_str());
        validate_proxy_for_storage(&proposed_user_config, validation_password)?;
    }

    // Phase 2: I/O. `inner.session.mutable` is not touched by any of this
    // -- a failure at any point here leaves it exactly as phase 1 read
    // it, so `settings()` keeps reporting the pre-patch values.
    if touches_socks5_identity {
        let Some(dbs) = current_dbs.clone() else {
            return Err(vault_unavailable_error());
        };
        let candidate = proposed_user_config.clone();
        let save_dbs = dbs.clone();
        let existing_password = existing_socks5_password.clone();
        let handle = inner
            .tokio_handle()
            .ok_or_else(shutdown_mid_request_error)?;
        handle
            .spawn_blocking(move || {
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

        // A patch that changes only the per-plugin proxy map (no SOCKS5
        // identity field -- `touches_socks5_identity` is `false`, which
        // is why this `else` arm ran at all) still needs to reach the
        // live `AsyncHttpClient`; the `if` arm above already covers this
        // as part of its own full `apply_live_proxy_routing` resolve.
        // This is the narrower equivalent for when only the map itself
        // changed: a plain in-memory swap, no vault/secrets access
        // needed, so it runs unconditionally rather than being
        // best-effort like `apply_live_proxy_routing`.
        if touches_plugin_proxy_map {
            let async_http_client = inner.core_services().async_http_client.clone();
            async_http_client.apply_plugin_proxy_map(
                arclain_core::utilities::proxy::effective_plugin_proxy_map(&proposed_user_config),
            );
        }
    }

    let vault_repoint = if touches_vault_paths {
        Some(
            repoint_vault_paths(
                inner,
                current_db_paths.clone(),
                default_db_paths,
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

    persist_app_config_policies(
        inner,
        &current_db_paths,
        &proposed_crc_policy,
        &proposed_collision_policy,
    )
    .await?;

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
    mutable.default_collision_policy = proposed_collision_policy;
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
        archive: settings::archive_dto(&mutable.user_config, &mutable.default_collision_policy),
        network: settings::network_dto(
            &mutable.user_config,
            socks5_password_configured,
            gameta_api_key_configured,
        ),
        security: settings::security_dto(&mutable),
        general: settings::general_dto(&mutable.user_config),
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

/// Health-checks a *candidate* gameta server configuration -- see
/// [`crate::ArclainApp::test_gameta_connection`] for the surface contract.
///
/// Deliberately does **not** take `settings_write_lock`, unlike every
/// other function in this module that touches gameta configuration: it
/// mutates nothing at all, so serializing it behind that lock would only
/// let a slow (up to `arclain_network::PROBE_TIMEOUT`) network probe
/// block unrelated settings saves, and let an unrelated save block the
/// user's "Test Connection" button, for no consistency benefit -- there
/// is no state here for a concurrent write to tear.
///
/// The blocking `GametaClient` runs on this application's own blocking
/// pool rather than inline, so a caller awaiting this from a
/// `current_thread` runtime never has its executor stalled for the
/// duration of the probe.
///
/// `api_key` is accepted but **not transmitted** -- see the facade
/// method's own doc comment for the full explanation and why that is
/// deliberately left as is here.
pub(super) async fn run_test_gameta_connection(
    inner: &Arc<AppRuntime>,
    server_url: String,
    api_key: Option<SecretInput>,
) -> Result<GametaServerInfo, ApplicationError> {
    let server_url = server_url.trim().to_string();
    if server_url.is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "a gameta server URL is required",
        )
        .with_recoverability(Recoverability::UserAction)
        .with_field("server_url"));
    }
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let probe_url = server_url.clone();
    // Exposed here and nowhere else: `ServerConfig` needs an owned
    // `String`, and this is the single grep-able point where the
    // candidate key leaves its zeroizing container. The copy lives only
    // as long as the client inside the closure below.
    let probe_key = api_key.map(|key| key.expose_secret().to_string());
    let health = handle
        .spawn_blocking(move || {
            arclain_network::features::gameta_client::GametaClient::new(
                arclain_network::features::gameta_client::ServerConfig {
                    url: probe_url,
                    api_key: probe_key,
                },
            )
            .health()
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| gameta_unreachable_error(&server_url, error))?;
    Ok(GametaServerInfo {
        status: health.status,
        version: health.version,
    })
}

/// Probes the *candidate* network path -- see
/// [`crate::ArclainApp::probe_network`] for the surface contract.
///
/// Takes no `settings_write_lock` and persists nothing, for the same
/// reasons [`run_test_gameta_connection`] does not.
///
/// `ProxyConfig::test_connection` is already `async` (it does its own
/// DNS, TCP, and HTTP steps through Tokio), so unlike the gameta probe
/// there is no blocking client to hand to `spawn_blocking`; the caller's
/// `dispatch_async` has already put this future on the application's own
/// runtime.
///
/// A probe that *ran* returns `Ok` whatever it found: the whole point of
/// the report is the trace, and a failed step is information to render,
/// not an error to swallow it. `Err` is reserved for a candidate that
/// could never have been probed (an unusable authority) or an
/// application on its way down.
pub(super) async fn run_probe_network(
    inner: &Arc<AppRuntime>,
    proxy: Option<Socks5Candidate>,
) -> Result<NetworkProbeReport, ApplicationError> {
    let candidate = match proxy {
        Some(proxy) => proxy_candidate(proxy)?,
        // `enabled: false` is `ProxyConfig::test_connection`'s direct
        // mode: it skips the DNS and TCP steps that only make sense for
        // a proxy and reports the egress of an unrouted request -- the
        // "what is my real IP without the proxy" half of the settings
        // page's test button.
        None => arclain_network::features::proxy::ProxyConfig {
            enabled: false,
            address: String::new(),
            username: None,
            password: None,
        },
    };
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };
    let result = handle
        .spawn(async move { candidate.test_connection().await })
        .await
        .map_err(internal_join_error)?;
    Ok(probe_report(result))
}

/// Turns a candidate into the `ProxyConfig` the probe runs on, rejecting
/// a host/port pair that could never form a usable authority *before* any
/// packet leaves -- and with a better message than the probe's own first
/// failed step would carry.
fn proxy_candidate(
    proxy: Socks5Candidate,
) -> Result<arclain_network::features::proxy::ProxyConfig, ApplicationError> {
    let host = proxy.host.trim().to_string();
    if host.is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "a proxy host is required",
        )
        .with_recoverability(Recoverability::UserAction)
        .with_field("host"));
    }
    if proxy.port == 0 {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "a proxy port is required",
        )
        .with_diagnostic("port 0 is not a connectable destination")
        .with_recoverability(Recoverability::UserAction)
        .with_field("port"));
    }
    // `enabled: true` is what makes this a *proxy* probe rather than the
    // direct one above: the candidate values only mean anything routed
    // through.
    let candidate = arclain_network::features::proxy::ProxyConfig {
        enabled: true,
        address: format!("{host}:{}", proxy.port),
        username: proxy.username,
        // The single grep-able point where the candidate password leaves
        // its zeroizing container, the same way the gameta key does above.
        password: proxy
            .password
            .map(|value| value.expose_secret().to_string()),
    };
    if let Err(reason) = candidate.validate() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "the proxy address is not usable",
        )
        .with_diagnostic(reason)
        .with_recoverability(Recoverability::UserAction)
        .with_field("host"));
    }
    Ok(candidate)
}

/// Mirrors a probe result into the report a frontend renders.
///
/// Every string carried across is already credential-free at its source:
/// `ConnectionTestStep::message` holds either an `io::Error` (a DNS or TCP
/// failure) or `ProxyConfig::log_summary`, which reports only enablement,
/// the host:port authority, and whether credentials were *present* --
/// never their values.
fn probe_report(result: ConnectionTestResult) -> NetworkProbeReport {
    let report = NetworkProbeReport {
        steps: result
            .steps
            .into_iter()
            .map(|step| ProbeStepDto {
                name: step.name,
                passed: step.passed,
                message: step.message,
            })
            .collect(),
        ip: result.ip,
        country: result.country,
    };
    // `NetworkProbeReport` carries no success flag of its own: a probe
    // stops at its first failed step, so "every step passed" is the same
    // verdict. Asserted rather than assumed, so a change to the probe's
    // own step bookkeeping shows up as a failing test here instead of a
    // panel that quietly disagrees with itself.
    debug_assert_eq!(
        report.succeeded(),
        result.success,
        "probe step trace disagrees with the probe's own success flag",
    );
    report
}

/// Sets or clears the SOCKS5 password.
///
/// Routed through the *same* journaled `NetworkProxyPersistenceService`
/// path `run_update_settings`'s identity-touching branch uses -- not a
/// bare `set_secret`/`remove_secret` -- because a standalone unjournaled
/// password write is unsafe on its own: if an *earlier, unrelated*
/// identity save (an address/username change) crashed after staging its
/// own recovery marker but before finalizing it, that marker's
/// `previous_password` snapshot predates whatever this call is about to
/// set. A later crash-recovery pass (at the next bootstrap) rolling
/// back that stale marker would otherwise silently overwrite this call's
/// password with the older snapshot, with no error and no warning.
/// Routing through `NetworkProxyPersistenceService::save` makes this
/// call's own `recover_pending()` run first (as `save`'s own first
/// step), resolving any such stale marker synchronously, right here,
/// before staging and committing the actual requested change -- so the
/// end state after this call returns is always the password this call
/// was actually asked to set, never a stale rollback snapshot.
///
/// Reads the *current on-disk* `user_config` row fresh -- not this
/// instance's cached copy -- before using it as `save`'s "candidate":
/// the same fix `run_update_settings` applies for the same reason (see
/// its own "C1" doc note). This call carries no settings patch of its
/// own (only the secret changes), but it is reachable directly through
/// the facade API on its own, not only after an `update_settings` call
/// that would have just refreshed the cache -- the egui `SaveNetwork`
/// handler always calls `update_settings` first, so this gap never
/// showed up through it, but a caller that invokes this method by
/// itself (a Flutter bridge, a script, any future frontend) would
/// otherwise silently overwrite every *other* column with whatever this
/// instance's cache last happened to see. The cache is refreshed from
/// this same read once the write succeeds, exactly like
/// `run_update_settings`'s own commit step -- and that same fresh row,
/// not the stale cached one, is what gets applied against the live
/// `AsyncHttpClient` below, so a plugin-proxy-map change nobody asked
/// this call to touch can never be silently reverted by it either.
pub(super) async fn run_set_socks5_password(
    inner: &Arc<AppRuntime>,
    value: Option<SecretInput>,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = {
        let mutable = inner.session.mutable.read();
        mutable.dbs.clone().ok_or_else(vault_unavailable_error)?
    };
    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;

    let read_config_service = config_service.clone();
    let user_config = handle
        .spawn_blocking(move || {
            read_config_service
                .get_user_config()
                .map_err(|error| backend_error("reading current settings", error))
        })
        .await
        .map_err(internal_join_error)??;

    let secret_value = value
        .as_ref()
        .map(|value| value.expose_secret().to_string());
    let save_dbs = dbs.clone();
    let candidate = user_config.clone();
    handle
        .spawn_blocking(move || {
            arclain_core::services::NetworkProxyPersistenceService::new(
                &config_service,
                &save_dbs.secrets,
            )
            .save(&candidate, secret_value.as_deref())
            .map_err(|error| persistence_error("saving the SOCKS5 password", error))
            .map(|_outcome| ())
        })
        .await
        .map_err(internal_join_error)??;

    {
        let mut mutable = inner.session.mutable.write();
        mutable.user_config = user_config.clone();
        mutable.revision += 1;
    }
    // Re-apply live routing with the NEW password immediately -- without
    // this, the new credential only takes effect after the next
    // `update_settings` call (which re-applies routing for identity
    // changes) or a restart, even though it is already correctly
    // persisted. See this module's own "I3" note. Uses the same fresh
    // `user_config` just committed above, not a stale cached one -- see
    // this function's own doc comment for why (the "NB1" fix).
    apply_live_proxy_routing(inner, &dbs, &user_config).await;
    Ok(())
}

pub(super) async fn run_move_vault(
    inner: &Arc<AppRuntime>,
    destination: PathBuf,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let current_db_paths = {
        let mutable = inner.session.mutable.read();
        mutable.db_paths.clone()
    };
    // Close the vault's `SecretsDb` for every outstanding clone of it --
    // not just this instance's own `mutable.dbs` -- before touching its
    // file. See `close_vault_handle`'s own doc comment for why clearing
    // only this instance's field is not enough on Windows.
    close_vault_handle(inner);

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
        let mutable = inner.session.mutable.read();
        mutable.db_paths.clone()
    };
    // See `run_move_vault`'s identical step and `close_vault_handle`'s
    // own doc comment -- `SecretsService::rekey_vault` additionally
    // *deletes* the old vault file, which fails outright on Windows
    // while any clone (this instance's own, or a long-lived external one
    // like `crates/ui`'s `AppState.dbs`) still has it open.
    close_vault_handle(inner);
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
                return Err(settings::password_required_for_new_rule_error());
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

pub(super) async fn run_replace_password_rules(
    inner: &Arc<AppRuntime>,
    edits: Vec<PasswordRuleEditInput>,
) -> Result<Vec<PasswordRuleSummary>, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;

    let (dbs, current_rules) = {
        let mutable = inner.session.mutable.read();
        let dbs = mutable.dbs.clone().ok_or_else(vault_unavailable_error)?;
        (dbs, mutable.pass_rules.clone())
    };
    let existing_names: std::collections::HashSet<&str> = current_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    settings::validate_password_rule_edit_inputs(&edits, &existing_names)?;

    let existing_by_name: std::collections::HashMap<&str, &arclain_core::PassRule> = current_rules
        .iter()
        .map(|rule| (rule.name.as_str(), rule))
        .collect();
    let mut replacement = Vec::with_capacity(edits.len());
    for edit in edits {
        let password = match edit.password {
            Some(secret) => secret.expose_secret().to_string(),
            None => {
                let original_name = edit
                    .original_name
                    .as_deref()
                    .ok_or_else(settings::password_required_for_new_rule_error)?;
                existing_by_name
                    .get(original_name)
                    .ok_or_else(settings::password_rule_original_name_not_found_error)?
                    .password
                    .clone()
            }
        };
        replacement.push(arclain_core::PassRule {
            name: edit.name,
            pattern: edit.pattern,
            password,
            priority: edit.priority,
            enabled: edit.enabled,
        });
    }

    persist_pass_rules(inner, &dbs, replacement.clone()).await?;

    let mut mutable = inner.session.mutable.write();
    mutable.pass_rules = replacement;
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

/// Enables or disables `plugin_id` on the live `PluginManager` and
/// persists the result so it survives a restart -- unlike
/// `arclain_plugins::PluginManager::enable_plugin`/`disable_plugin` on
/// their own (a plain in-memory `RwLock` write with nothing durable
/// underneath), this closes the loop bootstrap needs (see
/// `runtime::bootstrap::run`'s own "persisted enabled-plugin
/// reconciliation" step) to bring a disabled plugin back down on the
/// next launch.
///
/// Validates and applies the live toggle *first* -- an unknown
/// `plugin_id` fails with `NotFound` before anything is persisted.
///
/// Persists a full snapshot of every plugin's *actual* current `enabled`
/// state (`PluginSessionStore::plugins`, filtered), not an accumulated
/// add/remove diff against whatever `enabled_plugins` already held. An
/// accumulated diff cannot be reconstructed correctly at the next
/// bootstrap: a plugin nothing has ever toggled is absent from the diff
/// either way, indistinguishable from "the user explicitly disabled it".
/// A full snapshot has no such ambiguity -- absent means disabled, always
/// -- so bootstrap's reconciliation step can trust it completely.
pub(super) async fn run_set_plugin_enabled(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
    enabled: bool,
) -> Result<(), ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let manager = crate::plugins::require_manager(inner.plugin_manager())?;

    crate::plugins::PluginSessionStore::set_plugin_enabled(&manager, &plugin_id, enabled)?;

    let enabled_ids: Vec<String> = crate::plugins::PluginSessionStore::plugins(&manager, None)
        .into_iter()
        .filter(|summary| summary.enabled)
        .map(|summary| summary.id)
        .collect();

    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;

    // Re-read the current on-disk row fresh, matching `run_update_settings`'s
    // own "C1" rationale -- this call carries no patch of its own, only
    // the plugin-enabled snapshot, so every other column must survive
    // untouched.
    let read_config_service = config_service.clone();
    let mut candidate = handle
        .spawn_blocking(move || {
            read_config_service
                .get_user_config()
                .map_err(|error| backend_error("reading current settings", error))
        })
        .await
        .map_err(internal_join_error)??;
    candidate.set_enabled_plugins(&enabled_ids);

    let persisted = candidate.clone();
    handle
        .spawn_blocking(move || {
            config_service
                .save_user_config(&persisted)
                .map_err(|error| persistence_error("saving plugin enabled state", error))
        })
        .await
        .map_err(internal_join_error)??;

    let mut mutable = inner.session.mutable.write();
    mutable.user_config = candidate;
    mutable.revision += 1;
    Ok(())
}

/// Persists one plugin-domain approval and only then commits the same
/// decision to the live whitelist enforced by the HTTP client.
///
/// This shares `settings_write_lock` with the rest of the settings
/// mutations so two concurrent approve/revoke calls cannot land on disk
/// in one order and in memory in the opposite order. The database write
/// runs on the blocking pool; the live whitelist is changed only after
/// that write succeeds.
pub(super) async fn run_set_plugin_domain_approved(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
    domain: String,
    approved: bool,
) -> Result<(), ApplicationError> {
    let plugin_id = plugin_id.trim().to_string();
    if plugin_id.is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "plugin id must not be empty",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("plugin_id"));
    }
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "domain must not be empty",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("domain"));
    }

    let _write_guard = inner.settings_write_lock.lock().await;
    let whitelist = inner.core_services().domain_whitelist.clone();
    let requested = whitelist
        .read()
        .get_all_entries()
        .into_iter()
        .any(|entry| entry.plugin_id == plugin_id && entry.domain == domain);
    if !requested {
        return Err(ApplicationError::new(
            ApplicationErrorKind::NotFound,
            "plugin domain request not found",
        )
        .with_diagnostic(format!("plugin '{plugin_id}', domain '{domain}'"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("domain"));
    }
    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    let persisted_plugin_id = plugin_id.clone();
    let persisted_domain = domain.clone();
    handle
        .spawn_blocking(move || {
            let result = if approved {
                config_service.approve_plugin_domain(&persisted_plugin_id, &persisted_domain)
            } else {
                config_service.revoke_plugin_domain(&persisted_plugin_id, &persisted_domain)
            };
            result.map_err(|error| persistence_error("saving plugin domain approval", error))
        })
        .await
        .map_err(internal_join_error)??;

    let whitelist = whitelist.read();
    if approved {
        whitelist.approve(&plugin_id, &domain);
    } else {
        whitelist.revoke(&plugin_id, &domain);
        whitelist.add_pending(&plugin_id, &domain);
    }
    Ok(())
}

/// Pulls whatever `plugin_id`'s guest currently holds in its settings bag
/// out of the plugin runtime and persists it, unless it already matches
/// what is stored.
///
/// # Why this exists
///
/// A guest's `set-setting` host call writes into its own instance and
/// flips a dirty bit -- and stops there. Nothing in the plugin runtime
/// ever writes that to disk; the host has to come and take it. So every
/// call that enters a guest has to be followed by a pull, or the setting
/// lives only as long as the process.
///
/// Called after each guest entry on the session surface, which is where
/// settings are actually written: a plugin's settings form *is* a plugin
/// UI, so changing a setting is an ordinary `on-ui-event`.
///
/// # Why it compares before writing
///
/// [`run_flush_plugin_settings`] reads the whole user-config row, rewrites
/// it, and saves it back. Doing that on every plugin interaction, when
/// the overwhelming majority write no setting at all, would put a
/// full-row database round trip behind every button click. The comparison
/// is against this application's own mirror of the persisted row, which
/// is only ever advanced *after* a successful save -- so the mirror can
/// lag the disk but never lead it, and a stale mirror can only cause a
/// redundant write, never a skipped one.
///
/// # Failures are logged, not propagated
///
/// The caller is an operation whose real subject is the plugin's document
/// update. Failing that operation because a settings row could not be
/// written would discard a document the user is looking at in order to
/// report a problem with something else. The write is retried implicitly
/// by the next interaction: a failed save leaves the mirror untouched, so
/// that comparison still finds a difference next time.
pub(super) async fn flush_plugin_settings(inner: &Arc<AppRuntime>, plugin_id: &str) {
    if let Err(error) = run_flush_plugin_settings(inner, plugin_id.to_string()).await {
        tracing::error!(
            plugin_id,
            ?error,
            "failed to persist a plugin's settings after it ran"
        );
    }
}

/// Pulls **every** plugin's settings out of the plugin runtime and
/// persists whatever differs from what is stored, in one write. Called
/// from [`crate::ArclainApp::shutdown`], before the runtime tears down.
///
/// # Why a sweep at exit, when the per-plugin pull already converges
///
/// [`flush_plugin_settings`] runs after every guest entry on the *session*
/// surface, and the instance's dirty bit is sticky, so a write from any
/// other guest entry is normally picked up by the next session open or
/// dispatch. Normally -- but a process can exit before that next entry
/// ever happens, and then the write is simply lost. Three guest entries
/// have no pull of their own and are only reachable this way:
///
/// - `install_plugin_package`, whose approved package's `init` runs in the
///   guest. Install a plugin that records something at load, close the
///   application, and without this the record is gone.
/// - the top-tab query behind `plugin_chrome`, on a cache miss.
/// - the `OnArchiveOpen` event worker inside `arclain_plugins`, which runs
///   enabled guests with no plugin session involved at all -- the ordinary
///   shape of a command-line run.
///
/// This is also the one place a whole-map sweep is the right instrument:
/// at exit there is no way to know which plugins are dirty, which is
/// exactly what `PluginManager::get_all_settings` answers (and it is
/// dirty-bit aware, so clean plugins cost a cached clone, not a guest
/// call).
///
/// One row write for all plugins, not one per plugin: the per-plugin path
/// reads and rewrites the whole user-config row, which at exit would mean
/// two round trips per installed plugin for no benefit.
///
/// Merges into the stored map rather than replacing it, so a plugin that
/// is no longer loaded keeps whatever was last saved for it instead of
/// being silently dropped from the row.
pub(super) async fn run_flush_all_plugin_settings(inner: &Arc<AppRuntime>) {
    run_flush_all_plugin_settings_after(inner, std::future::ready(())).await;
}

async fn run_flush_all_plugin_settings_after(
    inner: &Arc<AppRuntime>,
    before_write_lock: impl std::future::Future<Output = ()>,
) {
    before_write_lock.await;
    let _write_guard = inner.settings_write_lock.lock().await;
    let Some(manager) = inner.plugin_manager() else {
        return;
    };
    let live = {
        let manager = manager.lock();
        manager.get_all_settings()
    };
    if live.is_empty() {
        return;
    }

    let Some(config_service) = inner.core_services().config_service.clone() else {
        return;
    };
    let Some(handle) = inner.tokio_handle() else {
        return;
    };

    let read_config_service = config_service.clone();
    let candidate = handle
        .spawn_blocking(move || read_config_service.get_user_config())
        .await;
    let mut candidate = match candidate {
        Ok(Ok(candidate)) => candidate,
        Ok(Err(error)) => {
            tracing::error!(%error, "could not read settings to flush plugin settings at exit");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "the plugin settings exit-flush worker failed");
            return;
        }
    };

    let mut stored = candidate.get_all_plugin_settings();
    let mut changed = false;
    for (plugin_id, settings) in live {
        if stored.get(&plugin_id) != Some(&settings) {
            stored.insert(plugin_id, settings);
            changed = true;
        }
    }
    if !changed {
        return;
    }
    candidate.set_all_plugin_settings(&stored);

    let persisted = candidate.clone();
    match handle
        .spawn_blocking(move || config_service.save_user_config(&persisted))
        .await
    {
        Ok(Ok(())) => {
            let mut mutable = inner.session.mutable.write();
            mutable.user_config = candidate;
            mutable.revision += 1;
        }
        Ok(Err(error)) => {
            tracing::error!(%error, "could not save plugin settings at exit")
        }
        Err(error) => tracing::error!(%error, "the plugin settings exit-flush worker failed"),
    }
}

/// Reads a loaded plugin's host-bounded settings and the application-wide
/// revision that protects a later replacement. Taking the same write lock as
/// mutations keeps the live map and revision from straddling a successful
/// compare-and-set activation.
pub(super) async fn run_plugin_settings(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
) -> Result<PluginSettingsSnapshot, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let manager = crate::plugins::require_manager(inner.plugin_manager())?;
    let values = {
        let manager = manager.lock();
        if manager.get_plugin_instance(&plugin_id).is_none() {
            return Err(crate::plugins::plugin_not_found(&plugin_id));
        }
        manager
            .get_settings_for(&plugin_id)
            .ok_or_else(|| crate::plugins::plugin_not_found(&plugin_id))?
    };
    let revision = inner.session.mutable.read().revision;
    Ok(PluginSettingsSnapshot {
        plugin_id,
        revision,
        values: values.into_iter().collect(),
    })
}

/// Persists a frontend-requested whole-map replacement only after its expected
/// shared revision, plugin liveness, and the host's canonical settings limits
/// have all been checked while `settings_write_lock` is held. The running
/// instance changes only after the user-config row saves successfully.
pub(super) async fn run_set_plugin_settings(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
    expected_revision: u64,
    values: BTreeMap<String, String>,
) -> Result<PluginSettingsSnapshot, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let current_revision = inner.session.mutable.read().revision;
    if expected_revision != current_revision {
        return Err(conflict_error(current_revision));
    }
    let validated = arclain_plugins::validate_plugin_settings(values.clone())
        .map_err(plugin_settings_validation_error)?;
    let manager = crate::plugins::require_manager(inner.plugin_manager())?;
    let replacement = {
        let manager = manager.lock();
        if manager.get_plugin_instance(&plugin_id).is_none() {
            return Err(crate::plugins::plugin_not_found(&plugin_id));
        }
        manager
            .prepare_plugin_settings_replacement(&plugin_id)
            .map_err(|error| plugin_settings_activation_error(&plugin_id, error))?
    };
    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;

    let read_config_service = config_service.clone();
    let original = handle
        .spawn_blocking(move || {
            read_config_service
                .get_user_config()
                .map_err(|error| backend_error("reading current settings", error))
        })
        .await
        .map_err(internal_join_error)??;
    let mut candidate = original.clone();
    candidate.set_plugin_settings(&plugin_id, HashMap::from_iter(values.clone()));

    let persisted = candidate.clone();
    let save_config_service = config_service.clone();
    handle
        .spawn_blocking(move || {
            save_config_service
                .save_user_config(&persisted)
                .map_err(|error| persistence_error("saving plugin settings", error))
        })
        .await
        .map_err(internal_join_error)??;

    let activation = {
        let mut manager = manager.lock();
        manager.replace_plugin_settings(replacement, validated)
    };
    if let Err(error) = activation {
        let rollback_service = config_service.clone();
        let rollback = handle
            .spawn_blocking(move || rollback_service.save_user_config(&original))
            .await;
        match rollback {
            Ok(Ok(())) => return Err(plugin_settings_activation_error(&plugin_id, error)),
            Ok(Err(rollback_error)) => {
                return Err(persistence_error(
                    "restoring plugin settings after live activation failed",
                    rollback_error,
                ))
            }
            Err(rollback_error) => return Err(internal_join_error(rollback_error)),
        }
    }

    let mut mutable = inner.session.mutable.write();
    mutable.user_config = candidate;
    mutable.revision += 1;
    Ok(PluginSettingsSnapshot {
        plugin_id,
        revision: mutable.revision,
        values,
    })
}

/// Persists guest-written settings after a guest entry. This intentionally
/// keeps the pre-CAS behavior: it trusts the running instance's already
/// bounded map, advances the shared revision after a successful save, and does
/// not try to reactivate the instance it just read from.
pub(super) async fn run_flush_plugin_settings(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
) -> Result<(), ApplicationError> {
    run_flush_plugin_settings_after(inner, plugin_id, std::future::ready(())).await
}

async fn run_flush_plugin_settings_after(
    inner: &Arc<AppRuntime>,
    plugin_id: String,
    before_write_lock: impl std::future::Future<Output = ()>,
) -> Result<(), ApplicationError> {
    before_write_lock.await;
    let _write_guard = inner.settings_write_lock.lock().await;
    let Some(manager) = inner.plugin_manager() else {
        return Ok(());
    };
    let settings = {
        let manager = manager.lock();
        manager.get_settings_for(&plugin_id)
    };
    let Some(settings) = settings else {
        return Ok(());
    };
    let already_persisted = {
        let mutable = inner.session.mutable.read();
        mutable.user_config.get_plugin_settings(&plugin_id) == settings
    };
    if already_persisted {
        return Ok(());
    }
    let config_service = inner
        .core_services()
        .config_service
        .clone()
        .ok_or_else(settings_unavailable_error)?;
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;

    let read_config_service = config_service.clone();
    let mut candidate = handle
        .spawn_blocking(move || {
            read_config_service
                .get_user_config()
                .map_err(|error| backend_error("reading current settings", error))
        })
        .await
        .map_err(internal_join_error)??;
    candidate.set_plugin_settings(&plugin_id, settings);

    let persisted = candidate.clone();
    handle
        .spawn_blocking(move || {
            config_service
                .save_user_config(&persisted)
                .map_err(|error| persistence_error("saving plugin settings", error))
        })
        .await
        .map_err(internal_join_error)??;

    let mut mutable = inner.session.mutable.write();
    mutable.user_config = candidate;
    mutable.revision += 1;
    Ok(())
}

// ============================================================================
// Shared helpers.
// ============================================================================

/// Re-applies SOCKS5 proxy routing (and the per-plugin proxy map) to this
/// instance's `AsyncHttpClient` after a settings save that touched a
/// SOCKS5 identity field -- mirrors the pre-facade `SettingsAction::
/// SaveNetwork` handler's own final step
/// (`shared.services.async_http_client.apply_proxy_routing(...)`), which
/// this replaces. `core_services().async_http_client` is the one
/// application-owned client used by the headless network consumers, so
/// applying the change here updates live routing without exposing that
/// client to a frontend.
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

/// Closes the current vault's `SecretsDb` for every outstanding clone of
/// it -- this instance's own `mutable.dbs`, `crates/ui`'s long-lived
/// `AppState.dbs` mirror, and anything else that took a
/// `LegacyComposition` earlier and never dropped it -- and clears
/// `mutable.dbs` to `None` here too, so `security_dto`'s
/// `vault_available` correctly reports the vault as unavailable for the
/// brief window between this call and whichever commit installs the new
/// one.
///
/// This is the fix for a real production bug: clearing only this
/// instance's own `mutable.dbs` field released *this* clone's reference
/// to the underlying `Arc<Mutex<Option<redb::Database>>>`, but a
/// long-lived external clone (obtained via `ArclainApp::
/// take_legacy_composition`, which `crates/ui` calls once at startup and
/// again after every settings mutation, and which is never dropped for
/// the app's entire lifetime) kept the file locked regardless -- Windows
/// refuses to copy (`move_vault`) or delete (`rekey_vault`) a file any
/// live handle still has open. `ReDb::close`/`SecretsDb::close` (see
/// their own doc comments) solve this by closing the *shared* underlying
/// database, which every clone -- including ones this function has never
/// heard of -- observes immediately, needing no cooperation from
/// whoever else is holding a reference. This restores the pre-facade
/// behavior of `AppState::move_vault`'s own `self.dbs.take()`: the one
/// live copy went dark the instant the move started; now every copy
/// does, together, the same way.
fn close_vault_handle(inner: &Arc<AppRuntime>) {
    let mut mutable = inner.session.mutable.write();
    if let Some(dbs) = mutable.dbs.take() {
        dbs.secrets.close();
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

/// Writes the two `app_config` key/value policies `update_settings` owns
/// -- the encrypted-CRC policy (security) and the pipeline collision
/// default (archive) -- in one connection, so a call that changes both
/// cannot land one and lose the other.
///
/// Writes both unconditionally, whether or not this patch changed either:
/// the value written is always the one the caller is about to commit
/// in-memory, so an unchanged field's write is a no-op re-write of what
/// is already stored. This is the behavior the CRC policy has had since
/// this surface existed; the collision policy simply joined it.
async fn persist_app_config_policies(
    inner: &Arc<AppRuntime>,
    db_paths: &Option<DbPaths>,
    encrypted_crc_policy: &str,
    default_collision_policy: &str,
) -> Result<(), ApplicationError> {
    let Some(db_paths) = db_paths.clone() else {
        return Ok(());
    };
    let crc_policy = encrypted_crc_policy.to_string();
    let collision_policy = default_collision_policy.to_string();
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
                    arclain_core::set_config(conn, "encrypted_crc_policy", &crc_policy)?;
                    arclain_core::set_config(
                        conn,
                        arclain_core::COLLISION_POLICY_CONFIG_KEY,
                        &collision_policy,
                    )
                })
                .map_err(|error| persistence_error("saving configuration policies", error))
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
fn patched_vault_paths(
    current_db_paths: Option<DbPaths>,
    default_db_paths: &DbPaths,
    patch: &crate::settings::SecuritySettingsPatch,
) -> DbPaths {
    let mut paths = current_db_paths.unwrap_or_else(|| default_db_paths.clone());
    settings::apply_vault_path_patch(&mut paths, patch, default_db_paths);
    paths
}

async fn repoint_vault_paths(
    inner: &Arc<AppRuntime>,
    current_db_paths: Option<DbPaths>,
    default_db_paths: DbPaths,
    patch: &crate::settings::SecuritySettingsPatch,
) -> Result<
    (
        arclain_core::DbPaths,
        arclain_core::ConfigDbs,
        Vec<arclain_core::PassRule>,
    ),
    ApplicationError,
> {
    let security_patch = patch.clone();
    let handle = inner
        .tokio_handle()
        .ok_or_else(shutdown_mid_request_error)?;
    handle
        .spawn_blocking(move || {
            // `Clear` on either field means "reset to `defaults`", not
            // "unset" -- see `settings::apply_vault_path_patch`'s own
            // doc comment (the "I6" fix) for why a vault path can never
            // simply be absent the way a directory override can.
            let paths = patched_vault_paths(current_db_paths, &default_db_paths, &security_patch);

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

/// Validates the SOCKS5 identity fields of a candidate `user_config`
/// against `current_password` -- the password that will actually remain
/// in effect after this call, since a patch that only touches the
/// identity fields (address/username/enabled) never changes the
/// password itself; that is
/// [`ArclainApp::set_socks5_password`](crate::runtime::ArclainApp::set_socks5_password)'s
/// own job. Passing the real current password here (instead of always
/// `None`) restores parity with the pre-facade `SaveNetwork` handler,
/// which always validated address+username+password together as one
/// `ProxyConfig` built from a single form submission --
/// `ProxyConfig::validate_for_storage` only exercises its
/// username+password-together branch (`proxy_url`'s `if let
/// (Some(username), Some(password)) = ...`) when both are `Some` at
/// once, so passing `password: None` unconditionally silently skipped
/// that branch for every call through this facade, even when a
/// username+password pair was actually configured.
fn validate_proxy_for_storage(
    user_config: &arclain_core::UserConfig,
    current_password: Option<&str>,
) -> Result<(), ApplicationError> {
    let config = arclain_network::features::proxy::ProxyConfig {
        enabled: user_config.socks5_enabled,
        address: user_config.socks5_address.clone().unwrap_or_default(),
        username: user_config.socks5_username.clone(),
        password: current_password.map(str::to_string),
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

/// Strips a URL's `user:password@` userinfo, leaving the rest intact.
///
/// A gameta server URL is user-typed configuration, not a secret, so
/// naming it in an error summary is what makes the failure actionable
/// ("which server did not answer?"). A URL carrying embedded credentials
/// is the one exception, and it is not hypothetical: this codebase
/// already refuses to *store* a SOCKS5 address with userinfo for the same
/// reason (`ProxyConfig::validate_for_storage`). Scrubbing it here keeps
/// the useful half of the URL in the message without turning a settings
/// form into a credential leak the moment a user pastes a URL from a
/// password manager.
///
/// Only userinfo is scrubbed out of a URL that parses. A query string is
/// left alone: a gameta base URL has no legitimate query component, so
/// redacting one would be guesswork about a shape this field never has,
/// and `with_diagnostic`'s own path-like-token redaction already removes
/// the full URL from the diagnostic channel regardless.
///
/// # Why a real parser, and not string splitting
///
/// Deciding where userinfo ends is a parsing problem, and the field this
/// runs on holds arbitrary typed text, not a validated URL. Splitting on
/// the first `://` and then on the last `@` gets every *well-formed* URL
/// right and still leaks on input like `user:pa://ss@host`, where the
/// leading `user:pa` is not a scheme at all but survives into the
/// message. So: only a string with no `@` anywhere is passed through
/// untouched (nothing to hide, provably), and anything else has to earn
/// its way out through `url::Url`. Input that does not parse, or parses
/// with no host to attribute the `@` to, is replaced wholesale --
/// over-redacting an unusable URL is cheaper than echoing a credential,
/// the same trade-off `ApplicationError::with_diagnostic`'s own
/// path-token pass already makes.
fn redact_url_userinfo(url: &str) -> String {
    // No '@' means no userinfo, whatever else the string is. This is the
    // overwhelmingly common case (including every ordinary typo), and it
    // keeps the message showing exactly what the user typed.
    if !url.contains('@') {
        return url.to_string();
    }
    let Ok(mut parsed) = url::Url::parse(url) else {
        return REDACTED_URL.to_string();
    };
    // No host means no authority, so nothing here can identify which side
    // of the '@' is a credential -- `user:pa://ss@host` parses as scheme
    // `user` plus an opaque path, with the whole credential inside it.
    if parsed.host().is_none() {
        return REDACTED_URL.to_string();
    }
    if parsed.username().is_empty() && parsed.password().is_none() {
        // The '@' is in the path or query, not the authority. Return the
        // original rather than `parsed.to_string()` so normalization
        // (an added trailing slash, for instance) does not silently
        // reword what the user typed.
        return url.to_string();
    }
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return REDACTED_URL.to_string();
    }
    parsed.to_string()
}

/// What [`redact_url_userinfo`] returns in place of a URL it cannot
/// safely take apart.
const REDACTED_URL: &str = "<redacted>";

/// The one failure shape [`run_test_gameta_connection`] reports: the
/// server named by `server_url` could not be reached, or answered its
/// health endpoint with something other than success.
///
/// The API key cannot appear in either channel: the summary is built from
/// `server_url` alone, and `with_diagnostic` receives only the client's
/// own error text (which carries the request URL at most -- redacted by
/// that method's path-like-token pass -- never a header value).
fn gameta_unreachable_error(server_url: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Backend,
        format!(
            "gameta server at {} did not answer a health check",
            redact_url_userinfo(server_url)
        ),
    )
    .with_diagnostic(error.to_string())
    .with_recoverability(Recoverability::Retry)
    .with_retryable(true)
    .with_suggested_action(SuggestedAction::Retry)
    .with_field("server_url")
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

fn plugin_settings_validation_error(
    _error: arclain_plugins::PluginSettingsValidationError,
) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "plugin settings exceed host limits",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_field("values")
}

fn plugin_settings_activation_error(
    plugin_id: &str,
    error: arclain_plugins::PluginError,
) -> ApplicationError {
    match error {
        arclain_plugins::PluginError::NotFound(_) => crate::plugins::plugin_not_found(plugin_id),
        error => ApplicationError::new(
            ApplicationErrorKind::Plugin,
            "failed to activate plugin settings",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::Retry),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_network::features::proxy::ConnectionTestStep;

    #[test]
    fn clear_uses_the_bootstrapped_profile_defaults() {
        use crate::settings::{PatchValue, SecuritySettingsPatch};

        let defaults = DbPaths::for_data_dir(std::path::Path::new("profile"));
        let mut current = defaults.clone();
        current.secrets_db = PathBuf::from("custom/pass.redb");
        current.key_file = Some(PathBuf::from("custom/master.key"));
        let patch = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Clear,
            key_file_path: PatchValue::Clear,
            encrypted_crc_policy: PatchValue::Keep,
        };

        let candidate = patched_vault_paths(Some(current.clone()), &defaults, &patch);

        assert_eq!(candidate.config_db, current.config_db);
        assert_eq!(candidate.cache_db, current.cache_db);
        assert_eq!(candidate.secrets_db, defaults.secrets_db);
        assert_eq!(candidate.key_file, defaults.key_file);
    }

    #[test]
    fn userinfo_redaction_leaves_an_ordinary_url_untouched() {
        for url in [
            "http://localhost:8080",
            "https://gameta.example/api",
            "https://gameta.example:8443/base/path",
            "gameta.example:8080",
        ] {
            assert_eq!(redact_url_userinfo(url), url);
        }
    }

    #[test]
    fn userinfo_redaction_strips_credentials_from_the_authority() {
        assert_eq!(
            redact_url_userinfo("https://user:hunter2@gameta.example/api"),
            "https://gameta.example/api",
        );
        assert_eq!(
            redact_url_userinfo("http://token@gameta.example:8443"),
            "http://gameta.example:8443/",
        );
    }

    /// A password containing '@' must not leave its tail behind: only the
    /// *last* '@' in the authority separates userinfo from the host.
    #[test]
    fn userinfo_redaction_splits_on_the_last_at_sign() {
        assert_eq!(
            redact_url_userinfo("https://user:p@ss@gameta.example/api"),
            "https://gameta.example/api",
        );
    }

    /// An '@' after the authority (in a path or query) belongs to the
    /// path, not to any credential, and must be left alone.
    #[test]
    fn userinfo_redaction_ignores_an_at_sign_outside_the_authority() {
        let url = "https://gameta.example/mail@example/inbox";
        assert_eq!(redact_url_userinfo(url), url);
    }

    /// Regression: userinfo containing `://` used to be read as a scheme,
    /// so the leading half of the credential (`user:pa`) was rebuilt into
    /// the summary as if it were `https`. Nothing here parses as a URL
    /// with a host, so nothing is echoed.
    #[test]
    fn userinfo_redaction_does_not_echo_a_credential_that_contains_a_scheme_separator() {
        for url in [
            "user:pa://ss@gameta.example",
            "https://user:pa://ss@gameta.example",
            "user:pa://ss@gameta.example/api?token=x",
        ] {
            let redacted = redact_url_userinfo(url);
            assert!(
                !redacted.contains("user:pa") && !redacted.contains("ss"),
                "{url} redacted to {redacted}",
            );
        }
    }

    /// A string that is not a URL at all, but does carry an `@`, cannot be
    /// taken apart safely -- so none of it is echoed.
    #[test]
    fn userinfo_redaction_replaces_an_unparsable_url_carrying_an_at_sign() {
        assert_eq!(
            redact_url_userinfo("not a url user:secret@host"),
            REDACTED_URL,
        );
    }

    fn step(name: &str, passed: bool, message: Option<&str>) -> ConnectionTestStep {
        ConnectionTestStep {
            name: name.to_string(),
            passed,
            message: message.map(str::to_string),
        }
    }

    /// Field-for-field fidelity against a constructed trace: the panel
    /// renders a row per step from exactly these three values, so
    /// summarizing, reordering, or dropping one would change what the
    /// user sees.
    #[test]
    fn probe_report_mirrors_every_step_of_a_successful_trace() {
        let source = ConnectionTestResult {
            steps: vec![
                step("DNS", true, Some("Resolved to 203.0.113.7:1080")),
                step("TCP", true, None),
                step("SOCKS5", true, None),
            ],
            success: true,
            ip: Some("198.51.100.9".to_string()),
            country: Some("Nowhere".to_string()),
        };

        let report = probe_report(source);

        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| (step.name.as_str(), step.passed, step.message.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("DNS", true, Some("Resolved to 203.0.113.7:1080")),
                ("TCP", true, None),
                ("SOCKS5", true, None),
            ],
        );
        assert_eq!(report.ip.as_deref(), Some("198.51.100.9"));
        assert_eq!(report.country.as_deref(), Some("Nowhere"));
        assert!(report.succeeded());
    }

    /// A probe stops at its first failed step, so the trace keeps the
    /// steps that did pass -- that partial progress is what tells a user
    /// where the path breaks.
    #[test]
    fn probe_report_keeps_the_passing_steps_that_preceded_a_failure() {
        let source = ConnectionTestResult {
            steps: vec![
                step("DNS", true, Some("Resolved to 203.0.113.7:1080")),
                step("TCP", false, Some("connection refused")),
            ],
            success: false,
            ip: None,
            country: None,
        };

        let report = probe_report(source);

        assert!(!report.succeeded());
        assert!(report.steps[0].passed);
        assert!(!report.steps[1].passed);
        assert_eq!(
            report.steps[1].message.as_deref(),
            Some("connection refused")
        );
    }

    /// An empty trace is not "everything passed". `succeeded` requires at
    /// least one step so a probe that produced nothing can never read as
    /// a success.
    #[test]
    fn an_empty_probe_report_does_not_read_as_a_success() {
        let report = NetworkProbeReport {
            steps: Vec::new(),
            ip: None,
            country: None,
        };

        assert!(!report.succeeded());
    }

    #[test]
    fn an_unreachable_server_error_names_the_host_but_never_the_credentials() {
        const PASSWORD: &str = "gameta-url-password-3f8c";

        let error = gameta_unreachable_error(
            &format!("https://operator:{PASSWORD}@gameta.example/api"),
            "Health request failed: connection refused",
        );

        let rendered = format!("{error:?}");
        assert!(!rendered.contains(PASSWORD), "{rendered}");
        assert!(!rendered.contains("operator"), "{rendered}");
        assert!(
            error.summary.contains("gameta.example"),
            "{}",
            error.summary
        );
        assert_eq!(error.kind, ApplicationErrorKind::Backend);
        assert!(error.retryable);
        assert_eq!(error.field.as_deref(), Some("server_url"));
    }

    const SETTINGS_RACE_PLUGIN_ID: &str = "ui-demo";

    fn bootstrap_settings_race_fixture(root: &std::path::Path) -> crate::ArclainApp {
        let paths = crate::AppPaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            log_dir: root.join("logs"),
            plugins_dir: root.join("plugins"),
        };
        let plugin_dir = paths.plugins_dir.join(SETTINGS_RACE_PLUGIN_ID);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../wirt/tests/fixtures/bundled/ui-demo.wasm"),
            plugin_dir.join("ui-demo.wasm"),
        )
        .unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../plugins/ui-demo/plugin.toml"),
            plugin_dir.join("ui-demo.toml"),
        )
        .unwrap();
        crate::ArclainApp::bootstrap(crate::BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: Some(2),
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        })
        .expect("bootstrap facade test fixture")
    }

    fn replace_live_plugin_settings(app: &crate::ArclainApp, values: BTreeMap<String, String>) {
        let manager = app.inner.plugin_manager().expect("plugin manager");
        let target = manager
            .lock()
            .prepare_plugin_settings_replacement(SETTINGS_RACE_PLUGIN_ID)
            .expect("loaded plugin generation");
        let validated = arclain_plugins::validate_plugin_settings(values).unwrap();
        manager
            .lock()
            .replace_plugin_settings(target, validated)
            .expect("replace live plugin settings");
    }

    #[derive(Clone, Copy)]
    enum SettingsFlushPath {
        Plugin,
        WholeMap,
    }

    fn assert_waiting_flush_cannot_overwrite_a_cas(path: SettingsFlushPath) {
        let temp = tempfile::tempdir().unwrap();
        let app = bootstrap_settings_race_fixture(temp.path());
        replace_live_plugin_settings(
            &app,
            BTreeMap::from([("source".to_string(), "guest-before-cas".to_string())]),
        );
        let caller_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let before = caller_runtime
            .block_on(app.plugin_settings(SETTINGS_RACE_PLUGIN_ID.to_string()))
            .expect("settings snapshot before CAS");
        let inner = app.inner.clone();
        let handle = inner.tokio_handle().expect("application runtime");
        let flush_inner = inner.clone();
        let (waiting_tx, waiting_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let flush = handle.spawn(async move {
            let pause = async move {
                waiting_tx.send(()).expect("race observer remains alive");
                release_rx.await.expect("race release remains alive");
            };
            match path {
                SettingsFlushPath::Plugin => {
                    run_flush_plugin_settings_after(
                        &flush_inner,
                        SETTINGS_RACE_PLUGIN_ID.to_string(),
                        pause,
                    )
                    .await
                    .expect("plugin settings flush");
                }
                SettingsFlushPath::WholeMap => {
                    run_flush_all_plugin_settings_after(&flush_inner, pause).await;
                }
            }
        });
        caller_runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), waiting_rx)
                .await
                .expect("flush must reach the write-lock boundary")
                .expect("flush must signal the write-lock boundary");
        });
        let cas = caller_runtime
            .block_on(app.set_plugin_settings(
                SETTINGS_RACE_PLUGIN_ID.to_string(),
                before.revision,
                BTreeMap::from([("source".to_string(), "cas".to_string())]),
            ))
            .expect("CAS must succeed while the older flush waits");
        release_tx.send(()).expect("flush remains alive");
        caller_runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), flush)
                .await
                .expect("flush must finish after release")
                .expect("flush task must not panic");
        });

        let after = caller_runtime
            .block_on(app.plugin_settings(SETTINGS_RACE_PLUGIN_ID.to_string()))
            .expect("settings snapshot after flush");
        assert_eq!(after.values, cas.values);
        assert_eq!(after.revision, cas.revision);
        let persisted = app
            .inner
            .core_services()
            .config_service
            .as_ref()
            .expect("configuration service")
            .get_user_config()
            .expect("persisted user config");
        assert_eq!(
            BTreeMap::from_iter(persisted.get_plugin_settings(SETTINGS_RACE_PLUGIN_ID)),
            cas.values,
        );
        caller_runtime.block_on(app.shutdown()).unwrap();
    }

    /// Catches the old ordering where a guest flush captured settings before
    /// waiting for `settings_write_lock`, then overwrote a newer CAS row after
    /// that CAS had already persisted and activated its replacement.
    #[test]
    fn guest_flush_snapshot_waiting_for_the_settings_lock_cannot_overwrite_a_cas() {
        assert_waiting_flush_cannot_overwrite_a_cas(SettingsFlushPath::Plugin);
    }

    /// The shutdown sweep is a distinct whole-map persistence path and must
    /// take its live snapshot only after admission to the same write lock.
    #[test]
    fn shutdown_flush_snapshot_waiting_for_settings_lock_cannot_overwrite_a_cas() {
        assert_waiting_flush_cannot_overwrite_a_cas(SettingsFlushPath::WholeMap);
    }
}
