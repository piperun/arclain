//! Password rules management operations
//!
//! Persistence goes through `arclain_app::ArclainApp`'s settings facade
//! (`upsert_password_rule`/`delete_password_rule`) -- see `vault_ops.rs`'s
//! own module doc comment for why these calls block briefly on `runtime`
//! rather than spawning fire-and-forget.
//!
//! `save_password_rules` diffs the incoming bulk list against the
//! facade's current one (matched by name) and issues the upsert/delete
//! calls needed to reach it, since the facade's contractual surface is
//! per-rule (`upsert_password_rule`/`delete_password_rule`), not a bulk
//! replace -- the settings page's rules dialog still edits a whole draft
//! list in memory (unchanged UX); only the *save* step's shape changed.

use super::config_ops::describe_facade_error;
use super::AppState;
use anyhow::Result;
use arclain_app::settings::PasswordRuleInput;
use arclain_app::ArclainApp;
use std::collections::HashSet;
use tokio::runtime::Runtime;

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
        rules: Vec<PasswordRuleInput>,
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
                    .upsert_password_rule(rule)
                    .await
                    .map_err(|error| describe_facade_error("saving a password rule", error))?;
            }
            Ok::<(), anyhow::Error>(())
        })?;

        self.refresh_settings_from_facade(facade, runtime)
    }
}
