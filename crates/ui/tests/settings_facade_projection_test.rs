mod common;

use arclain_ui::core::SettingsPage;
use arclain_ui::features::settings::types::{DropBehavior, EncryptedCrcPolicy, SettingsAction};
use arclain_ui::features::settings::types::{
    WIRT_PLUGIN_FILTER_NAME, WIRT_PLUGIN_FILTER_SUFFIXES, WIRT_PLUGIN_PICKER_TITLE,
};
use arclain_ui::features::settings::{SettingsFeature, SettingsFeatureRefs};

fn no_cross_feature_refs() -> SettingsFeatureRefs<'static> {
    SettingsFeatureRefs {
        password_management: None,
    }
}

#[test]
fn plugin_picker_offers_only_wirt_packages() {
    assert_eq!(WIRT_PLUGIN_PICKER_TITLE, "Select Wirt Plugin to Install");
    assert_eq!(WIRT_PLUGIN_FILTER_NAME, "Wirt Plugin");
    assert_eq!(WIRT_PLUGIN_FILTER_SUFFIXES, &["wirt"]);
}

#[test]
fn plugin_picker_action_preserves_the_exact_os_path() {
    #[cfg(windows)]
    let package_path = {
        use std::os::windows::ffi::OsStringExt as _;
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&[
            b'p' as u16,
            b'l' as u16,
            b'u' as u16,
            b'g' as u16,
            b'i' as u16,
            b'n' as u16,
            0xd800,
            b'.' as u16,
            b'w' as u16,
            b'i' as u16,
            b'r' as u16,
            b't' as u16,
        ]))
    };
    #[cfg(unix)]
    let package_path = {
        use std::os::unix::ffi::OsStringExt as _;
        std::path::PathBuf::from(std::ffi::OsString::from_vec(b"plugin\xff.wirt".to_vec()))
    };

    let action = SettingsAction::InspectPluginPackage {
        package_path: package_path.clone(),
    };
    let SettingsAction::InspectPluginPackage {
        package_path: retained,
    } = action
    else {
        panic!("constructed the inspection action")
    };
    assert_eq!(retained, package_path);
}

#[test]
fn untouched_settings_forms_match_the_facade_snapshot() {
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
