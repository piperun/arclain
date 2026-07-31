mod common;

use arclain_ui::core::SettingsPage;
use arclain_ui::features::settings::types::{DropBehavior, EncryptedCrcPolicy};
use arclain_ui::features::settings::{SettingsFeature, SettingsFeatureRefs};

fn no_cross_feature_refs() -> SettingsFeatureRefs<'static> {
    SettingsFeatureRefs {
        password_management: None,
    }
}

#[test]
fn untouched_settings_forms_match_the_facade_snapshot_not_legacy_state() {
    let shared = common::create_test_shared_state();

    let mut general = arclain_app::settings::GeneralSettingsDto::default();
    general.open_nested_in_new_tab = true;
    general.drop_behavior = "ask_each_time".to_string();
    general.restore_tabs_on_launch = false;
    shared.signals().general_settings.set(general);

    let mut archive = arclain_app::settings::ArchiveSettingsDto::default();
    archive.temp_directory = Some(std::path::PathBuf::from("facade-temp"));
    shared.signals().archive_settings.set(archive);

    let mut network = arclain_app::settings::NetworkSettingsDto::default();
    network.socks5_enabled = true;
    network.socks5_address = Some("proxy.example:1080".to_string());
    network.socks5_username = Some("facade-user".to_string());
    network.gameta_server_enabled = true;
    network.gameta_server_url = Some("https://gameta.example".to_string());
    shared.signals().network_settings.set(network);

    let mut security = arclain_app::settings::SecuritySettingsDto::default();
    security.encrypted_crc_policy = "prompt_on_open".to_string();
    shared.signals().security_settings.set(security);

    // Deliberately contradict every facade-backed value. During the
    // transition this mirror can lag behind; it must not drive a form.
    {
        let mut legacy = shared.app_state.lock();
        legacy.user_config.open_nested_in_new_tab = false;
        legacy.user_config.drop_behavior = Some("new_tab".to_string());
        legacy.user_config.restore_tabs_on_launch = true;
        legacy.user_config.temp_dir = None;
        legacy.user_config.socks5_enabled = false;
        legacy.user_config.socks5_address = None;
        legacy.user_config.socks5_username = None;
        legacy.user_config.gameta_server_enabled = false;
        legacy.user_config.gameta_server_url = None;
    }

    let feature = SettingsFeature::new(&shared);

    assert!(*feature.general_state.open_nested_in_new_tab.read());
    assert_eq!(
        *feature.general_state.drop_behavior.read(),
        DropBehavior::AskEachTime
    );
    assert!(!*feature.general_state.restore_tabs_on_launch.read());
    assert_eq!(&*feature.archives_state.temp_dir.read(), "facade-temp");
    assert_eq!(
        *feature.security_state.encrypted_crc_policy.read(),
        EncryptedCrcPolicy::PromptOnOpen
    );

    for page in [
        SettingsPage::General,
        SettingsPage::Archives,
        SettingsPage::Network,
        SettingsPage::Server,
        SettingsPage::Security,
    ] {
        assert!(
            !feature.check_changes(&shared, &page, no_cross_feature_refs()),
            "an untouched {page:?} form must not be dirty"
        );
    }

    feature
        .network_state
        .socks5_password
        .set("replacement-proxy-secret".to_string());
    feature
        .server_state
        .api_key
        .set("replacement-api-key".to_string());
    assert!(feature.check_changes(&shared, &SettingsPage::Network, no_cross_feature_refs()));
    assert!(feature.check_changes(&shared, &SettingsPage::Server, no_cross_feature_refs()));
}

#[test]
fn settings_forms_never_receive_persisted_secrets() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let facade = shared.facade.as_ref().expect("test facade");
    let snapshot = shared.services.tokio_runtime.block_on(async {
        facade
            .set_socks5_password(Some(arclain_app::challenge::SecretInput::new(
                "stored-proxy-secret".to_string(),
            )))
            .await
            .expect("seed proxy secret through facade");
        facade
            .set_gameta_api_key(arclain_app::challenge::SecretInput::new(
                "stored-gameta-secret".to_string(),
            ))
            .await
            .expect("seed gameta secret through facade");
        facade.settings().await.expect("refresh settings snapshot")
    });
    shared.signals().network_settings.set(snapshot.network);

    let feature = SettingsFeature::new(&shared);

    assert!(feature.network_state.socks5_password_configured);
    assert!(feature.server_state.api_key_configured);
    assert!(feature.network_state.socks5_password.read().is_empty());
    assert!(feature.server_state.api_key.read().is_empty());
}
