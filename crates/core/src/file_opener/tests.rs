use super::*;

#[test]
fn test_same_directory_extraction() {
    let opener = FileOpener::new().unwrap();
    let all_files = vec![
        "game/game.exe".to_string(),
        "game/config.ini".to_string(),
        "game/data.dat".to_string(),
        "game/subfolder/texture.png".to_string(),
        "readme.txt".to_string(),
    ];

    let files =
        opener.get_files_to_extract("game/game.exe", &all_files, OpenStrategy::SameDirectory);

    // Should include all files in game/ and its subdirectories
    assert!(files.contains(&"game/game.exe".to_string()));
    assert!(files.contains(&"game/config.ini".to_string()));
    assert!(files.contains(&"game/data.dat".to_string()));
    assert!(files.contains(&"game/subfolder/texture.png".to_string()));
    // Should NOT include files outside the directory
    assert!(!files.contains(&"readme.txt".to_string()));
}

#[test]
fn test_root_directory_extraction() {
    let opener = FileOpener::new().unwrap();
    let all_files = vec![
        "app.exe".to_string(),
        "config.txt".to_string(),
        "folder/file.dat".to_string(),
    ];

    let files = opener.get_files_to_extract("app.exe", &all_files, OpenStrategy::SameDirectory);

    // Should include all files since target is in root
    assert_eq!(files.len(), 3);
}
