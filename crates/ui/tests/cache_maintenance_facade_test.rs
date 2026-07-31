mod common;

use arclain_ui::features::settings::actions::handle_action;
use arclain_ui::features::settings::types::{
    ArchivesSettingsState, NetworkSettingsState, SecuritySettingsState, ServerSettingsState,
    SettingsAction,
};

#[test]
fn cache_content_clear_uses_the_facade_cache_not_legacy_database_paths() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let cache_root = &shared
        .facade
        .as_ref()
        .expect("test facade")
        .paths()
        .cache_dir;
    let content_dir = cache_root.join("content-v2");
    let resources_dir = cache_root.join("resources");
    std::fs::create_dir_all(&content_dir).unwrap();
    std::fs::create_dir_all(&resources_dir).unwrap();
    std::fs::write(content_dir.join("blob"), b"cached").unwrap();
    std::fs::write(resources_dir.join("keep"), b"resource").unwrap();

    // Prove the action needs no legacy database/path mirror at all.
    {
        let mut legacy = shared.app_state.lock();
        legacy.dbs = None;
        legacy.db_paths = None;
    }

    handle_action(
        SettingsAction::ClearCacheContent,
        &mut SecuritySettingsState::default(),
        &mut ArchivesSettingsState::default(),
        None,
        &mut NetworkSettingsState::default(),
        &mut ServerSettingsState::default(),
        &shared,
    );

    assert!(!content_dir.exists());
    assert!(resources_dir.join("keep").is_file());
}
