mod common;

use arclain_app::challenge::SecretInput;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::settings::PasswordRuleInput;
use arclain_ui::features::password_management::domain::types::PasswordRule;
use arclain_ui::features::password_management::PasswordManagementFeature;

fn seed_rule(shared: &arclain_ui::shared::SharedState, name: &str, password: &str) {
    let facade = shared.facade.as_ref().expect("test facade");
    shared
        .services
        .tokio_runtime
        .block_on(facade.upsert_password_rule(PasswordRuleInput {
            name: name.to_string(),
            pattern: "fixture-pattern".to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new(password.to_string())),
        }))
        .expect("seed password rule");
}

#[test]
fn saved_password_is_projected_as_configured_but_never_enters_frontend_state() {
    const SECRET: &str = "must-stay-behind-facade-164c";
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    seed_rule(&shared, "saved rule", SECRET);

    let mut feature = PasswordManagementFeature::new(&shared);
    feature.reload(&shared).expect("reload summaries");

    assert_eq!(feature.password_rules_dialog.rules.len(), 1);
    let row = &feature.password_rules_dialog.rules[0];
    assert_eq!(row.original_name.as_deref(), Some("saved rule"));
    assert!(row.password_configured);
    assert!(row.replacement_password.is_empty());
    assert!(!format!("{row:?}").contains(SECRET));
    assert!(!feature.is_dirty());
}

#[test]
fn blank_password_rename_saves_and_resets_the_non_secret_baseline() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    seed_rule(&shared, "before rename", "preserved-by-rust-f8aa");

    let mut feature = PasswordManagementFeature::new(&shared);
    feature.reload(&shared).expect("reload summaries");
    feature.password_rules_dialog.rules[0].name = "after rename".to_string();
    feature.password_rules_dialog.rules[0].pattern = "renamed-pattern".to_string();

    assert!(feature.is_dirty());
    feature.save(&shared).expect("save renamed rule");

    assert!(!feature.is_dirty());
    let row = &feature.password_rules_dialog.rules[0];
    assert_eq!(row.original_name.as_deref(), Some("after rename"));
    assert_eq!(row.name, "after rename");
    assert!(row.password_configured);
    assert!(row.replacement_password.is_empty());

    let summaries = shared
        .services
        .tokio_runtime
        .block_on(
            shared
                .facade
                .as_ref()
                .expect("test facade")
                .password_rules(),
        )
        .expect("read saved rules");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "after rename");
    assert_eq!(summaries[0].pattern, "renamed-pattern");
    assert!(summaries[0].password_configured);
}

#[test]
fn new_rule_without_password_is_rejected_and_keeps_the_existing_draft_dirty() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let mut feature = PasswordManagementFeature::new(&shared);
    feature.reload(&shared).expect("load empty baseline");
    feature.password_rules_dialog.rules.push(PasswordRule {
        original_name: None,
        name: "new rule".to_string(),
        pattern: "new-pattern".to_string(),
        replacement_password: String::new(),
        password_configured: false,
        priority: 10,
        enabled: true,
    });

    let error = feature
        .save(&shared)
        .expect_err("new rule without password must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("password"));
    assert!(feature.is_dirty());
    assert_eq!(feature.password_rules_dialog.rules.len(), 1);
    assert!(shared
        .services
        .tokio_runtime
        .block_on(
            shared
                .facade
                .as_ref()
                .expect("test facade")
                .password_rules()
        )
        .unwrap()
        .is_empty());
}
