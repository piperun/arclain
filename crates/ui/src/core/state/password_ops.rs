//! Password rules management operations
//!
//! Persistence now goes through `arclain_app::ArclainApp`'s settings
//! facade (`upsert_password_rule`/`delete_password_rule`) instead of
//! writing `dbs.secrets` directly -- see `vault_ops.rs`'s own module doc
//! comment for why these calls block briefly on `runtime` rather than
//! spawning fire-and-forget.
//!
//! `save_password_rules` diffs the incoming bulk list against the
//! facade's current one (matched by name) and issues the upsert/delete
//! calls needed to reach it, since the facade's contractual surface is
//! per-rule (`upsert_password_rule`/`delete_password_rule`), not a bulk
//! replace -- the settings page's rules dialog still edits a whole
//! `Vec<PassRule>` draft in memory (unchanged UX); only the *save* step's
//! shape changed.

use super::config_ops::describe_facade_error;
use super::AppState;
use anyhow::Result;
use arclain_app::challenge::SecretInput;
use arclain_app::settings::PasswordRuleInput;
use arclain_app::ArclainApp;
use arclain_core::utilities::password_matcher::derive_pattern_for;
use arclain_core::utilities::PassRule;
use std::collections::HashSet;
use std::path::Path;
use tokio::runtime::Runtime;
use tracing::info;

fn to_rule_input(rule: PassRule) -> PasswordRuleInput {
    PasswordRuleInput {
        name: rule.name,
        pattern: rule.pattern,
        priority: rule.priority,
        enabled: rule.enabled,
        password: Some(SecretInput::new(rule.password)),
    }
}

impl AppState {
    /// Reconciles the settings page's whole in-memory rule list against
    /// the facade: upserts every rule present in `rules`, then deletes
    /// every currently-stored rule whose name is no longer present.
    /// Matched by name -- the same identity a rename-via-edit already
    /// had under the pre-facade bulk `replace_all_pass_rules` (there was
    /// never a separate stable id), so a rename reaches the same end
    /// state (old name gone, new name present) either way.
    pub fn save_password_rules(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        rules: Vec<PassRule>,
    ) -> Result<()> {
        runtime.block_on(async {
            let current = facade
                .password_rules()
                .await
                .map_err(|error| describe_facade_error("reading current password rules", error))?;

            let incoming_names: HashSet<&str> =
                rules.iter().map(|rule| rule.name.as_str()).collect();
            for stale in current
                .iter()
                .filter(|rule| !incoming_names.contains(rule.name.as_str()))
            {
                facade
                    .delete_password_rule(stale.name.clone())
                    .await
                    .map_err(|error| {
                        describe_facade_error("deleting a removed password rule", error)
                    })?;
            }

            for rule in rules {
                facade
                    .upsert_password_rule(to_rule_input(rule))
                    .await
                    .map_err(|error| describe_facade_error("saving a password rule", error))?;
            }
            Ok::<(), anyhow::Error>(())
        })?;

        self.refresh_settings_from_facade(facade)
    }

    /// Auto-saves (or updates) a password rule after a successful
    /// archive unlock.
    ///
    /// **Currently unreachable**: no call site in this crate constructs
    /// the archive-open success path this exists for anymore. It was
    /// called from the pre-facade synchronous `list_with_password`
    /// success handler in `core::operations::archive`; that handler was
    /// removed when archive opening moved onto `ArclainApp::
    /// start_open_archive` (driven by `core::operation_bridge`), and no
    /// equivalent call was added to the new async flow. The `PasswordDialog::
    /// save_password` toggle users see ("Save password for future use")
    /// is similarly disconnected today -- rendered, but never read by
    /// anything. Both are pre-existing gaps, not introduced by this
    /// change; restoring the auto-save UX is a UI-flow decision (where in
    /// `dialog_handler.rs`/`operation_bridge.rs` to call this, and
    /// whether to gate it on the toggle) outside this task's settings/
    /// secrets/vault scope. This function is migrated to the facade
    /// (rather than deleted) so a restored call site finds a correct
    /// implementation waiting.
    pub fn save_password_rule_from_archive(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        archive_path: &Path,
        password: &str,
    ) -> Result<()> {
        let filename = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if filename.is_empty() {
            return Ok(());
        }

        // Derive the broadest reasonable pattern from the filename.
        // See `derive_pattern_for` for the heuristic — RJ/VJ/BJ code
        // first, then leading [Maker] bracket, then literal-filename
        // fallback. The old behavior used regex::escape(filename)
        // unconditionally, which produced a rule matching exactly
        // one archive — bad for the common case where a user has
        // multiple archives from the same source sharing a password.
        let pattern = derive_pattern_for(filename);

        info!(
            "Auto-saving password rule for archive '{}' with pattern '{}'",
            filename, pattern
        );

        runtime.block_on(async {
            let current = facade
                .password_rules()
                .await
                .map_err(|error| describe_facade_error("reading current password rules", error))?;
            // Match by pattern (not name): a second archive in the same
            // RJ-code-or-bracket family collides with an existing rule
            // instead of creating a redundant duplicate. The synthesized
            // "Auto-saved: <file>" name below is per-archive and would
            // never match a prior auto-save's name even when the
            // pattern does, so matching on name here would defeat the
            // dedup this is for.
            let name = current
                .iter()
                .find(|rule| rule.pattern == pattern)
                .map(|rule| rule.name.clone())
                .unwrap_or_else(|| format!("Auto-saved: {filename}"));

            facade
                .upsert_password_rule(PasswordRuleInput {
                    name,
                    pattern,
                    priority: 10,
                    enabled: true,
                    password: Some(SecretInput::new(password.to_string())),
                })
                .await
                .map_err(|error| describe_facade_error("auto-saving a password rule", error))
                .map(|_rules| ())
        })?;

        self.refresh_settings_from_facade(facade)
    }
}
