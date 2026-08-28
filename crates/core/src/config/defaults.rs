use crate::features::organization::presets::{mod_manager_layout, product_layout};
use crate::features::organization::{OrganizationRule, RuleActions, RuleTrigger};

/// The rules `sync::sync_rules` seeds an empty database with.
///
/// `database::ensure_default_rules` seeds the same table with a
/// different product rule and runs first, so on a first run this set
/// only lands if that one failed. Both ship both layouts all the same:
/// whichever seeder gets there, a fresh install must be able to organize
/// a mod pack, and a layout is data a rule carries — the rules editor
/// does not write one, so a rule carrying the mod-manager layout either
/// arrives seeded or never exists.
pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![
        OrganizationRule {
            name: "DLSite Archive".to_string(),
            priority: 100,
            is_enabled: true,
            trigger: RuleTrigger {
                metadata_source: Some("dlsite".to_string()),
                filename_pattern: Some(r"\[(RJ|BJ|VJ)\d+\]".to_string()),
                has_file: None,
            },
            actions: RuleActions {
                output_name: None,
                layout: product_layout("[$product_id][$circle] $title"),
            },
            ..Default::default()
        },
        // Every folder holding a `modinfo.ini` becomes its own output.
        // Lower priority than the product rule: a storefront product
        // that happens to ship mods inside it is still a product.
        OrganizationRule {
            name: "Mod Manager Layout".to_string(),
            priority: 50,
            is_enabled: true,
            trigger: RuleTrigger {
                metadata_source: None,
                filename_pattern: None,
                has_file: Some("modinfo.ini".to_string()),
            },
            actions: RuleActions {
                output_name: None,
                layout: mod_manager_layout(),
            },
            ..Default::default()
        },
    ]
}
