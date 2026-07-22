use super::*;
use tempfile::TempDir;

fn create_test_config() -> ConfigStore {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("config.json");
    ConfigStore {
        path,
        cfg: Config::default(),
    }
}

#[test]
fn test_archive_filename_matching() {
    let mut store = create_test_config();

    // Add a rule that matches [TestSite] archives
    store.cfg.pass_rules.push(PassRule {
        name: "TestSite".to_string(),
        pattern: r"\[TestSite\].+\.(rar|zip)".to_string(),
        password: "test_pass".to_string(),
        priority: 10,
        enabled: true,
    });

    // Test with Japanese filenames covering hiragana, katakana, and kanji
    // Hiragana: あいうえお | Katakana: テスト | Kanji: 試験作品
    let result = store.auto_password_for(
        Some("[TestSite] [RJ999001] 試験ゲームあいうえお.rar"), // kanji + katakana + hiragana
        &vec!["game.exe".to_string(), "data/scene1.dat".to_string()],
    );
    assert_eq!(result, Some("test_pass".to_string()));

    let result2 = store.auto_password_for(
        Some("[TestSite] テスト・RPG 64bit.rar"), // katakana with middle dot
        &vec!["game.exe".to_string()],
    );
    assert_eq!(result2, Some("test_pass".to_string()));
}

#[test]
fn test_archive_filename_not_matching() {
    let mut store = create_test_config();

    store.cfg.pass_rules.push(PassRule {
        name: "TestSite".to_string(),
        pattern: r"\[TestSite\].+\.(rar|zip)".to_string(),
        password: "test_pass".to_string(),
        priority: 10,
        enabled: true,
    });

    // Should not match other publishers
    let result = store.auto_password_for(
        Some("[OtherSite] ゲーム作品.rar"), // katakana + kanji, different site
        &vec!["game.exe".to_string()],
    );
    assert_eq!(result, None);
}

#[test]
fn test_internal_file_matching() {
    let mut store = create_test_config();

    // Rule that matches internal file paths
    store.cfg.pass_rules.push(PassRule {
        name: "Data files".to_string(),
        pattern: r".*\.dat$".to_string(),
        password: "data_pass".to_string(),
        priority: 10,
        enabled: true,
    });

    // Should match based on internal files
    let result = store.auto_password_for(
        Some("archive.zip"),
        &vec!["game.exe".to_string(), "data/scene1.dat".to_string()],
    );
    assert_eq!(result, Some("data_pass".to_string()));
}

#[test]
fn test_priority_ordering() {
    let mut store = create_test_config();

    // Add rules with different priorities
    store.cfg.pass_rules.push(PassRule {
        name: "Low priority".to_string(),
        pattern: r".*\.rar".to_string(),
        password: "low_pass".to_string(),
        priority: 5,
        enabled: true,
    });

    store.cfg.pass_rules.push(PassRule {
        name: "High priority".to_string(),
        pattern: r"\[TestSite\].*\.rar".to_string(),
        password: "high_pass".to_string(),
        priority: 20,
        enabled: true,
    });

    // Should match the higher priority rule first
    let result = store.auto_password_for(Some("[TestSite] サンプル作品.rar"), &vec![]); // katakana + kanji
    assert_eq!(result, Some("high_pass".to_string()));
}

#[test]
fn test_disabled_rule_not_matched() {
    let mut store = create_test_config();

    store.cfg.pass_rules.push(PassRule {
        name: "Disabled".to_string(),
        pattern: r".*\.rar".to_string(),
        password: "disabled_pass".to_string(),
        priority: 10,
        enabled: false, // Disabled
    });

    let result = store.auto_password_for(Some("file.rar"), &vec![]);
    assert_eq!(result, None);
}

#[test]
fn test_invalid_regex_pattern() {
    let mut store = create_test_config();

    // Add a rule with invalid regex
    store.cfg.pass_rules.push(PassRule {
        name: "Invalid".to_string(),
        pattern: "[invalid regex(".to_string(),
        password: "pass".to_string(),
        priority: 10,
        enabled: true,
    });

    // Should not crash, just skip the invalid rule
    let result = store.auto_password_for(Some("file.rar"), &vec![]);
    assert_eq!(result, None);
}

#[test]
fn test_archive_path_extracted_from_full_path() {
    let mut store = create_test_config();

    store.cfg.pass_rules.push(PassRule {
        name: "Test".to_string(),
        pattern: r"\[TestSite\].*".to_string(),
        password: "test_pass".to_string(),
        priority: 10,
        enabled: true,
    });

    // Should extract filename from full Windows path (hiragana: たなかさん)
    let result = store.auto_password_for(
        Some(r"C:\Users\Test\Downloads\[TestSite] たなかさんの冒険.rar"),
        &vec![],
    );
    assert_eq!(result, Some("test_pass".to_string()));

    // Should also work with Unix paths (kanji + katakana)
    let result2 = store.auto_password_for(
        Some("/home/user/downloads/[TestSite] 冒険ゲーム.rar"),
        &vec![],
    );
    assert_eq!(result2, Some("test_pass".to_string()));
}
