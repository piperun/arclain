use crate::organization::engine::RuleEngine;
use crate::ArchiveEntry;

#[cfg(test)]
mod pruning_tests {
    use super::*;

    /// Helper to create an ArchiveEntry
    fn make_entry(path: &str, size: u64, is_dir: bool) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_string(),
            size,
            packed_size: size,
            modified: None,
            is_dir,
            encrypted: false,
            crc32: None,
        }
    }

    #[test]
    fn test_remove_zero_byte_files() {
        let entries = vec![
            make_entry("game/data.txt", 1024, false),
            make_entry("game/empty.txt", 0, false),
            make_entry("game/config.json", 512, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should have 2 files (empty.txt removed)
        assert_eq!(pruned.len(), 2);
        assert!(pruned.iter().any(|e| e.path == "game/data.txt"));
        assert!(pruned.iter().any(|e| e.path == "game/config.json"));
        assert!(!pruned.iter().any(|e| e.path == "game/empty.txt"));
    }

    #[test]
    fn test_remove_empty_directories() {
        let entries = vec![
            make_entry("game/data/", 0, true),
            make_entry("game/data/file.txt", 1024, false),
            make_entry("game/empty_dir/", 0, true),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should have game/data/ and file, but not empty_dir/
        assert!(pruned.iter().any(|e| e.path.contains("data")));
        assert!(!pruned.iter().any(|e| e.path == "game/empty_dir/"));
    }

    #[test]
    fn test_nested_empty_folders() {
        let entries = vec![
            make_entry("wrapper/", 0, true),
            make_entry("wrapper/empty1/", 0, true),
            make_entry("wrapper/empty1/empty2/", 0, true),
            make_entry("wrapper/game/", 0, true),
            make_entry("wrapper/game/file.exe", 2048, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should keep wrapper/game/file.exe
        // Should remove empty1 and its children (all empty)
        assert!(pruned.iter().any(|e| e.path.contains("file.exe")));
        assert!(!pruned.iter().any(|e| e.path.contains("empty1")));
        assert!(!pruned.iter().any(|e| e.path.contains("empty2")));
    }

    #[test]
    fn test_preserve_platform_specific_files() {
        let entries = vec![
            make_entry("game/__MACOSX/", 0, true),
            make_entry("game/__MACOSX/resource.fork", 256, false),
            make_entry("game/.DS_Store", 128, false),
            make_entry("game/Thumbs.db", 64, false),
            make_entry("game/data.txt", 1024, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All files should be kept (no junk filtering)
        assert!(pruned.iter().any(|e| e.path.contains("__MACOSX")));
        assert!(pruned.iter().any(|e| e.path.contains(".DS_Store")));
        assert!(pruned.iter().any(|e| e.path.contains("Thumbs.db")));
        assert!(pruned.iter().any(|e| e.path.contains("data.txt")));
    }

    #[test]
    fn test_preserve_folder_structure() {
        let entries = vec![
            make_entry("root/swiftshader/", 0, true),
            make_entry("root/swiftshader/libEGL.dll", 512, false),
            make_entry("root/swiftshader/libGLESv2.dll", 1024, false),
            make_entry("root/locales/", 0, true),
            make_entry("root/locales/en-US.pak", 256, false),
            make_entry("root/locales/ja.pak", 128, false),
            make_entry("root/game.exe", 2048, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All structure should be preserved
        assert!(pruned.iter().any(|e| e.path.contains("swiftshader")));
        assert!(pruned.iter().any(|e| e.path.contains("locales")));
        assert!(pruned.iter().any(|e| e.path == "root/game.exe"));

        // Check specific files exist
        assert!(pruned
            .iter()
            .any(|e| e.path == "root/swiftshader/libEGL.dll"));
        assert!(pruned.iter().any(|e| e.path == "root/locales/en-US.pak"));
    }

    #[test]
    fn test_mixed_scenario() {
        let entries = vec![
            // Valid game structure
            make_entry("Game/data/", 0, true),
            make_entry("Game/data/maps.dat", 4096, false),
            make_entry("Game/audio/", 0, true),
            make_entry("Game/audio/bgm.ogg", 2048, false),
            make_entry("Game/game.exe", 8192, false),
            // Empty files to remove
            make_entry("Game/empty.txt", 0, false),
            make_entry("Game/data/empty.dat", 0, false),
            // Empty directories to remove
            make_entry("Game/unused/", 0, true),
            make_entry("Game/unused/nested/", 0, true),
            // Directory that becomes empty after file removal
            make_entry("Game/temp/", 0, true),
            make_entry("Game/temp/zero.tmp", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Check valid files are kept
        assert!(pruned.iter().any(|e| e.path == "Game/data/maps.dat"));
        assert!(pruned.iter().any(|e| e.path == "Game/audio/bgm.ogg"));
        assert!(pruned.iter().any(|e| e.path == "Game/game.exe"));

        // Check empty files are removed
        assert!(!pruned.iter().any(|e| e.path == "Game/empty.txt"));
        assert!(!pruned.iter().any(|e| e.path == "Game/data/empty.dat"));

        // Check empty directories are removed
        assert!(!pruned.iter().any(|e| e.path.contains("unused")));
        assert!(!pruned.iter().any(|e| e.path.contains("temp")));
    }

    #[test]
    fn test_deeply_nested_structure() {
        let entries = vec![
            make_entry("a/", 0, true),
            make_entry("a/b/", 0, true),
            make_entry("a/b/c/", 0, true),
            make_entry("a/b/c/d/", 0, true),
            make_entry("a/b/c/d/file.txt", 100, false),
            make_entry("a/b/empty/", 0, true),
            make_entry("a/x/", 0, true),
            make_entry("a/x/data.bin", 200, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should keep the deep path with file
        assert!(pruned.iter().any(|e| e.path == "a/b/c/d/file.txt"));
        assert!(pruned.iter().any(|e| e.path == "a/x/data.bin"));

        // Should remove empty branch
        assert!(!pruned.iter().any(|e| e.path == "a/b/empty/"));
    }

    #[test]
    fn test_all_empty() {
        let entries = vec![
            make_entry("empty1/", 0, true),
            make_entry("empty2/", 0, true),
            make_entry("file1.txt", 0, false),
            make_entry("file2.txt", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Everything should be removed
        assert_eq!(pruned.len(), 0);
    }

    #[test]
    fn test_single_file_at_root() {
        let entries = vec![make_entry("game.exe", 1024, false)];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should keep the single file
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].path, "game.exe");
    }

    #[test]
    fn test_real_world_game_structure() {
        // Simulating the structure from the user's screenshot
        let entries = vec![
            make_entry("swiftshader/", 0, true),
            make_entry("swiftshader/libEGL.dll", 3149824, false),
            make_entry("swiftshader/libGLESv2.dll", 1019152, false),
            make_entry("locales/", 0, true),
            make_entry("locales/en-US.pak", 47302369, false),
            make_entry("locales/ja.pak", 4945223, false),
            make_entry("js/", 0, true),
            make_entry("js/libs/", 0, true),
            make_entry("js/libs/pixi.js", 924976, false),
            make_entry("img/", 0, true),
            make_entry("img/pictures/", 0, true),
            make_entry("img/pictures/pic001.png", 216491071, false),
            make_entry("icon/", 0, true),
            make_entry("icon/icon.png", 72778, false),
            make_entry("fonts/", 0, true),
            make_entry("fonts/mplus.ttf", 1014504, false),
            make_entry("effects/", 0, true),
            make_entry("effects/effect1.webm", 243383, false),
            make_entry("data/", 0, true),
            make_entry("data/System.json", 5489001, false),
            make_entry("css/", 0, true),
            make_entry("css/game.css", 2188, false),
            make_entry("audio/", 0, true),
            make_entry("audio/bgm/", 0, true),
            make_entry("audio/bgm/theme.ogg", 95875690, false),
            make_entry("Game.exe", 1024, false),
            make_entry("package.json", 512, false),
            make_entry("readme.txt", 256, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All folders should be preserved
        assert!(pruned.iter().any(|e| e.path.contains("swiftshader")));
        assert!(pruned.iter().any(|e| e.path.contains("locales")));
        assert!(pruned.iter().any(|e| e.path.contains("js")));
        assert!(pruned.iter().any(|e| e.path.contains("img")));
        assert!(pruned.iter().any(|e| e.path.contains("icon")));
        assert!(pruned.iter().any(|e| e.path.contains("fonts")));
        assert!(pruned.iter().any(|e| e.path.contains("effects")));
        assert!(pruned.iter().any(|e| e.path.contains("data")));
        assert!(pruned.iter().any(|e| e.path.contains("css")));
        assert!(pruned.iter().any(|e| e.path.contains("audio")));

        // All files should be kept
        assert!(pruned.iter().any(|e| e.path == "Game.exe"));
        assert!(pruned.iter().any(|e| e.path == "package.json"));
        assert!(pruned.iter().any(|e| e.path == "readme.txt"));
    }

    #[test]
    fn test_japanese_game_structure() {
        // Real structure from user's game
        let entries = vec![
            // Directories
            make_entry("audio/", 0, true),
            make_entry("css/", 0, true),
            make_entry("data/", 0, true),
            make_entry("fonts/", 0, true),
            make_entry("icon/", 0, true),
            make_entry("img/", 0, true),
            make_entry("js/", 0, true),
            make_entry("locales/", 0, true),
            make_entry("save/", 0, true),
            make_entry("swiftshader/", 0, true),
            // Files
            make_entry("credits.html", 4689679, false),
            make_entry("d3dcompiler_47.dll", 4524696, false),
            make_entry("ffmpeg.dll", 1736704, false),
            make_entry("icudtl.dat", 10518160, false),
            make_entry("index.html", 674, false),
            make_entry("libEGL.dll", 386560, false),
            make_entry("libGLESv2.dll", 8192512, false),
            make_entry("node.dll", 12323840, false),
            make_entry("notification_helper.exe", 919552, false),
            make_entry("nw.dll", 143216128, false),
            make_entry("nw_100_percent.pak", 846670, false),
            make_entry("nw_200_percent.pak", 1491056, false),
            make_entry("nw_elf.dll", 901120, false),
            make_entry("package.json", 316, false),
            make_entry("Remove.bat", 876, false),
            make_entry("resources.pak", 6193027, false),
            make_entry("Tool.bat", 1100, false),
            make_entry("bin", 1838709, false),
            make_entry("031607P.bin", 1838709, false),
            make_entry("v8_context_snapshot.bin", 171496, false),
            make_entry("ReadMe.txt", 6634, false),
            make_entry("クエスト.exe", 2139648, false),
            // Files in subdirectories
            make_entry("audio/bgm.ogg", 2048000, false),
            make_entry("css/game.css", 1024, false),
            make_entry("data/maps.json", 512000, false),
            make_entry("fonts/font.ttf", 256000, false),
            make_entry("icon/icon.png", 128000, false),
            make_entry("img/title.png", 1024000, false),
            make_entry("js/main.js", 64000, false),
            make_entry("locales/ja.pak", 32000, false),
            make_entry("save/save01.sav", 16000, false),
            make_entry("swiftshader/libEGL.dll", 512000, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All directories and files should be preserved
        assert!(pruned.iter().any(|e| e.path.contains("audio")));
        assert!(pruned.iter().any(|e| e.path.contains("save")));
        assert!(pruned.iter().any(|e| e.path == "ReadMe.txt"));
        assert!(pruned.iter().any(|e| e.path == "クエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "031607P.bin"));
    }

    #[test]
    fn test_unicode_and_special_chars() {
        let entries = vec![
            make_entry("游戏/文件.txt", 1024, false),
            make_entry("Spiel/Datei.exe", 2048, false),
            make_entry("jeu/fichier (copie).dat", 512, false),
            make_entry("game [v1.0]/data.bin", 256, false),
            make_entry("ファイル/データ.json", 128, false),
            make_entry("папка/файл.txt", 64, false),
            // Empty file with unicode name
            make_entry("空のファイル.empty", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All non-empty files should be kept
        assert_eq!(pruned.len(), 6);
        assert!(pruned.iter().any(|e| e.path == "游戏/文件.txt"));
        assert!(pruned.iter().any(|e| e.path == "ファイル/データ.json"));
        assert!(!pruned.iter().any(|e| e.path == "空のファイル.empty"));
    }

    #[test]
    fn test_deeply_nested_with_sparse_files() {
        let entries = vec![
            // Deep path with file at the end
            make_entry("a/", 0, true),
            make_entry("a/b/", 0, true),
            make_entry("a/b/c/", 0, true),
            make_entry("a/b/c/d/", 0, true),
            make_entry("a/b/c/d/e/", 0, true),
            make_entry("a/b/c/d/e/f/", 0, true),
            make_entry("a/b/c/d/e/f/deep_file.txt", 100, false),
            // Another branch with empty folders
            make_entry("a/b/empty1/", 0, true),
            make_entry("a/b/empty1/empty2/", 0, true),
            make_entry("a/b/empty1/empty2/empty3/", 0, true),
            // Branch with file in middle
            make_entry("a/x/", 0, true),
            make_entry("a/x/y/", 0, true),
            make_entry("a/x/y/file.dat", 200, false),
            make_entry("a/x/y/z/", 0, true),
            make_entry("a/x/y/z/empty/", 0, true),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should keep the deep paths with files
        assert!(pruned.iter().any(|e| e.path == "a/b/c/d/e/f/deep_file.txt"));
        assert!(pruned.iter().any(|e| e.path == "a/x/y/file.dat"));

        // Should remove all empty branches
        assert!(!pruned.iter().any(|e| e.path.contains("empty1")));
        assert!(!pruned.iter().any(|e| e.path.contains("empty2")));
        assert!(!pruned.iter().any(|e| e.path.contains("empty3")));
        assert!(!pruned.iter().any(|e| e.path == "a/x/y/z/"));
    }

    #[test]
    fn test_alternating_empty_non_empty() {
        let entries = vec![
            make_entry("root/", 0, true),
            make_entry("root/level1_file.txt", 100, false),
            make_entry("root/empty/", 0, true),
            make_entry("root/nonempty/", 0, true),
            make_entry("root/nonempty/file.dat", 200, false),
            make_entry("root/nonempty/empty_child/", 0, true),
            make_entry("root/another_empty/", 0, true),
            make_entry("root/another_nonempty/", 0, true),
            make_entry("root/another_nonempty/data.bin", 300, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Keep files and their parent directories
        assert!(pruned.iter().any(|e| e.path == "root/level1_file.txt"));
        assert!(pruned.iter().any(|e| e.path == "root/nonempty/file.dat"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "root/another_nonempty/data.bin"));

        // Remove empty directories
        assert!(!pruned.iter().any(|e| e.path == "root/empty/"));
        assert!(!pruned.iter().any(|e| e.path == "root/another_empty/"));
        assert!(!pruned
            .iter()
            .any(|e| e.path == "root/nonempty/empty_child/"));
    }

    #[test]
    fn test_single_file_in_deep_hierarchy() {
        let entries = vec![
            make_entry("wrapper/", 0, true),
            make_entry("wrapper/intermediate/", 0, true),
            make_entry("wrapper/intermediate/subfolder/", 0, true),
            make_entry("wrapper/intermediate/subfolder/data/", 0, true),
            make_entry(
                "wrapper/intermediate/subfolder/data/important.file",
                1024,
                false,
            ),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Should keep only the path to the file
        assert_eq!(pruned.len(), 1);
        assert_eq!(
            pruned[0].path,
            "wrapper/intermediate/subfolder/data/important.file"
        );
    }

    #[test]
    fn test_many_siblings_with_one_empty() {
        let entries = vec![
            make_entry("parent/", 0, true),
            make_entry("parent/child1/", 0, true),
            make_entry("parent/child1/file1.txt", 100, false),
            make_entry("parent/child2/", 0, true),
            make_entry("parent/child2/file2.txt", 200, false),
            make_entry("parent/child3/", 0, true), // Empty
            make_entry("parent/child4/", 0, true),
            make_entry("parent/child4/file4.txt", 400, false),
            make_entry("parent/child5/", 0, true),
            make_entry("parent/child5/file5.txt", 500, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All non-empty children should be kept
        assert!(pruned.iter().any(|e| e.path == "parent/child1/file1.txt"));
        assert!(pruned.iter().any(|e| e.path == "parent/child2/file2.txt"));
        assert!(pruned.iter().any(|e| e.path == "parent/child4/file4.txt"));
        assert!(pruned.iter().any(|e| e.path == "parent/child5/file5.txt"));

        // Empty child should be removed
        assert!(!pruned.iter().any(|e| e.path == "parent/child3/"));
    }

    #[test]
    fn test_files_with_zero_and_nonzero_at_same_level() {
        let entries = vec![
            make_entry("dir/", 0, true),
            make_entry("dir/empty1.txt", 0, false),
            make_entry("dir/valid.txt", 100, false),
            make_entry("dir/empty2.dat", 0, false),
            make_entry("dir/another.bin", 200, false),
            make_entry("dir/empty3.log", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Only non-empty files should remain
        assert_eq!(pruned.len(), 2);
        assert!(pruned.iter().any(|e| e.path == "dir/valid.txt"));
        assert!(pruned.iter().any(|e| e.path == "dir/another.bin"));
        assert!(!pruned.iter().any(|e| e.size == 0 && !e.is_dir));
    }

    #[test]
    fn test_cascade_empty_removal() {
        // Directory becomes empty after its children are removed
        let entries = vec![
            make_entry("root/", 0, true),
            make_entry("root/branch1/", 0, true),
            make_entry("root/branch1/subbranch/", 0, true),
            make_entry("root/branch1/subbranch/empty.txt", 0, false),
            make_entry("root/branch2/", 0, true),
            make_entry("root/branch2/valid.dat", 100, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Only branch2 and its file should remain
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].path, "root/branch2/valid.dat");
    }

    #[test]
    fn test_root_level_files_only() {
        let entries = vec![
            make_entry("file1.exe", 1024, false),
            make_entry("file2.dll", 2048, false),
            make_entry("readme.txt", 512, false),
            make_entry("config.json", 256, false),
            make_entry("empty.log", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        assert_eq!(pruned.len(), 4);
        assert!(pruned.iter().all(|e| !e.is_dir));
        assert!(!pruned.iter().any(|e| e.path == "empty.log"));
    }

    #[test]
    fn test_game_plus_image_pack_folder() {
        // Game structure + separate image pack folder
        let entries = vec![
            // Game files
            make_entry("game.exe", 2048, false),
            make_entry("data.pak", 1024, false),
            // Image pack folder (sibling to game files)
            make_entry("ImagePack/", 0, true),
            make_entry("ImagePack/image001.png", 512000, false),
            make_entry("ImagePack/image002.png", 256000, false),
            make_entry("ImagePack/readme.txt", 128, false),
            // Some empty junk to remove
            make_entry("empty_folder/", 0, true),
            make_entry("zero_file.tmp", 0, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // All legitimate files should be kept
        assert!(pruned.iter().any(|e| e.path == "game.exe"));
        assert!(pruned.iter().any(|e| e.path == "data.pak"));
        assert!(pruned.iter().any(|e| e.path == "ImagePack/image001.png"));
        assert!(pruned.iter().any(|e| e.path == "ImagePack/image002.png"));
        assert!(pruned.iter().any(|e| e.path == "ImagePack/readme.txt"));

        // Junk should be removed
        assert!(!pruned.iter().any(|e| e.path == "zero_file.tmp"));
        assert!(!pruned.iter().any(|e| e.path.contains("empty_folder")));
    }

    #[test]
    fn test_game_plus_image_pack_archive() {
        // Game structure + image pack as a separate archive file
        let entries = vec![
            // Game directory
            make_entry("Game/", 0, true),
            make_entry("Game/game.exe", 2048, false),
            make_entry("Game/data/", 0, true),
            make_entry("Game/data/maps.dat", 1024, false),
            // Image pack as a zip file (sibling to Game folder)
            make_entry("ImagePack.zip", 5000000, false),
        ];

        let pruned = RuleEngine::prune_entries(&entries);

        // Both game files and image pack should be kept
        assert!(pruned.iter().any(|e| e.path == "Game/game.exe"));
        assert!(pruned.iter().any(|e| e.path == "Game/data/maps.dat"));
        assert!(pruned.iter().any(|e| e.path == "ImagePack.zip"));
    }

    /// Helper to create the full Japanese game structure
    fn make_japanese_game(prefix: &str) -> Vec<ArchiveEntry> {
        let p = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix)
        };
        vec![
            // Root level files
            make_entry(&format!("{}credits.html", p), 4689679, false),
            make_entry(&format!("{}d3dcompiler_47.dll", p), 4524696, false),
            make_entry(&format!("{}ffmpeg.dll", p), 1736704, false),
            make_entry(&format!("{}icudtl.dat", p), 10518160, false),
            make_entry(&format!("{}index.html", p), 674, false),
            make_entry(&format!("{}libEGL.dll", p), 386560, false),
            make_entry(&format!("{}libGLESv2.dll", p), 8192512, false),
            make_entry(&format!("{}node.dll", p), 12323840, false),
            make_entry(&format!("{}notification_helper.exe", p), 919552, false),
            make_entry(&format!("{}nw.dll", p), 143216128, false),
            make_entry(&format!("{}nw_100_percent.pak", p), 846670, false),
            make_entry(&format!("{}nw_200_percent.pak", p), 1491056, false),
            make_entry(&format!("{}nw_elf.dll", p), 901120, false),
            make_entry(&format!("{}package.json", p), 316, false),
            make_entry(&format!("{}Remove Tool files from game.bat", p), 876, false),
            make_entry(&format!("{}resources.pak", p), 6193027, false),
            make_entry(&format!("{}StartWithTool.bat", p), 1100, false),
            make_entry(&format!("{}TrsData.bin", p), 1838709, false),
            make_entry(
                &format!("{}TrsData_Bing_2025-11-02 031607P.bin", p),
                1838709,
                false,
            ),
            make_entry(&format!("{}v8_context_snapshot.bin", p), 171496, false),
            make_entry(
                &format!("{}はじめにお読み下さい_ReadMe.txt", p),
                6634,
                false,
            ),
            make_entry(
                &format!("{}聖女マグノリアのクエスト.exe", p),
                2139648,
                false,
            ),
            // Subdirectories with files
            make_entry(&format!("{}audio/bgm.ogg", p), 2048000, false),
            make_entry(&format!("{}audio/se.ogg", p), 512000, false),
            make_entry(&format!("{}css/game.css", p), 1024, false),
            make_entry(&format!("{}data/maps.json", p), 512000, false),
            make_entry(&format!("{}data/system.json", p), 256000, false),
            make_entry(&format!("{}fonts/font.ttf", p), 256000, false),
            make_entry(&format!("{}icon/icon.png", p), 128000, false),
            make_entry(&format!("{}img/title.png", p), 1024000, false),
            make_entry(&format!("{}img/background.png", p), 512000, false),
            make_entry(&format!("{}js/main.js", p), 64000, false),
            make_entry(&format!("{}js/plugins.js", p), 32000, false),
            make_entry(&format!("{}locales/en-US.pak", p), 47302369, false),
            make_entry(&format!("{}locales/ja.pak", p), 4945223, false),
            make_entry(&format!("{}save/save01.sav", p), 16000, false),
            make_entry(&format!("{}swiftshader/libEGL.dll", p), 512000, false),
            make_entry(&format!("{}swiftshader/libGLESv2.dll", p), 256000, false),
        ]
    }

    #[test]
    fn test_wave_direct_game_structure() {
        // Archive -> game structure (no nesting)
        let mut entries = make_japanese_game("");

        // Add some junk
        entries.push(make_entry("empty.tmp", 0, false));
        entries.push(make_entry("junk/", 0, true));

        let pruned = RuleEngine::prune_entries(&entries);

        // All game files should be preserved
        assert!(pruned
            .iter()
            .any(|e| e.path == "聖女マグノリアのクエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "audio/bgm.ogg"));
        assert!(pruned.iter().any(|e| e.path == "swiftshader/libEGL.dll"));

        // Junk removed
        assert!(!pruned.iter().any(|e| e.path == "empty.tmp"));

        // Should have all game files (38 files total from helper)
        assert_eq!(pruned.len(), 38);
    }

    #[test]
    fn test_wave_nested_depth_2_game_structure() {
        // Archive -> nested/nested -> game structure
        let mut entries = make_japanese_game("wrapper1/wrapper2");

        // Add junk at different levels
        entries.push(make_entry("wrapper1/empty/", 0, true));
        entries.push(make_entry("wrapper1/wrapper2/junk.tmp", 0, false));
        entries.push(make_entry("trash/", 0, true));

        let pruned = RuleEngine::prune_entries(&entries);

        // All game files preserved with their nested paths
        assert!(pruned
            .iter()
            .any(|e| e.path == "wrapper1/wrapper2/聖女マグノリアのクエスト.exe"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "wrapper1/wrapper2/audio/bgm.ogg"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "wrapper1/wrapper2/save/save01.sav"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "wrapper1/wrapper2/swiftshader/libGLESv2.dll"));

        // Junk removed
        assert!(!pruned.iter().any(|e| e.path.contains("empty")));
        assert!(!pruned.iter().any(|e| e.path.contains("junk")));
        assert!(!pruned.iter().any(|e| e.path.contains("trash")));

        assert_eq!(pruned.len(), 38);
    }

    #[test]
    fn test_wave_nested_depth_5_game_structure() {
        // Archive -> 5 levels deep -> game structure
        let mut entries = make_japanese_game("a/b/c/d/e");

        // Add empty branches at various depths
        entries.push(make_entry("a/empty1/", 0, true));
        entries.push(make_entry("a/b/empty2/", 0, true));
        entries.push(make_entry("a/b/c/empty3/", 0, true));
        entries.push(make_entry("a/b/c/d/empty4/", 0, true));
        entries.push(make_entry("zero.file", 0, false));

        let pruned = RuleEngine::prune_entries(&entries);

        // Game preserved at depth 5
        assert!(pruned
            .iter()
            .any(|e| e.path == "a/b/c/d/e/聖女マグノリアのクエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "a/b/c/d/e/locales/ja.pak"));
        assert!(pruned.iter().any(|e| e.path == "a/b/c/d/e/img/title.png"));

        // All empty branches removed
        assert!(!pruned.iter().any(|e| e.path.contains("empty")));
        assert!(!pruned.iter().any(|e| e.path == "zero.file"));

        assert_eq!(pruned.len(), 38);
    }

    #[test]
    fn test_wave_random_pattern_game_plus_imagepack() {
        use rand::Rng;
        let mut rng = rand::rng();

        // Test multiple random iterations to ensure robustness
        for iteration in 0..5 {
            let mut entries = vec![];

            // Randomly choose depths (0-5 levels)
            let game_depth: usize = rng.random_range(0..=5);
            let imagepack_depth: usize = rng.random_range(0..=5);

            // Build random path for game
            let game_prefix = if game_depth == 0 {
                String::new()
            } else {
                let mut path = String::new();
                for i in 0..game_depth {
                    if i > 0 {
                        path.push('/');
                    }
                    path.push_str(&format!("nest{}", i));
                }
                path
            };

            // Build random path for ImagePack
            let imagepack_prefix = if imagepack_depth == 0 {
                String::new()
            } else {
                let mut path = String::new();
                for i in 0..imagepack_depth {
                    if i > 0 {
                        path.push('/');
                    }
                    path.push_str(&format!("wrapper{}", i));
                }
                path
            };

            // Add game structure at random depth
            entries.extend(make_japanese_game(&game_prefix));

            // Add ImagePack at random depth
            let img_base = if imagepack_prefix.is_empty() {
                "ImagePack".to_string()
            } else {
                format!("{}/ImagePack", imagepack_prefix)
            };
            entries.push(make_entry(
                &format!("{}/wallpaper01.png", img_base),
                1024000,
                false,
            ));
            entries.push(make_entry(
                &format!("{}/wallpaper02.png", img_base),
                512000,
                false,
            ));
            entries.push(make_entry(&format!("{}/readme.txt", img_base), 256, false));

            // Add random junk at various depths
            for i in 0..rng.random_range(3..8) {
                let junk_depth = rng.random_range(0..=4);
                let mut junk_path = String::new();
                for j in 0..junk_depth {
                    if j > 0 {
                        junk_path.push('/');
                    }
                    junk_path.push_str(&format!("junk{}", j));
                }
                if junk_depth > 0 {
                    junk_path.push('/');
                }
                junk_path.push_str(&format!("empty{}.tmp", i));
                entries.push(make_entry(&junk_path, 0, false));
            }

            // Add random empty folders
            for _i in 0..rng.random_range(2..5) {
                let empty_depth = rng.random_range(1..=3);
                let mut empty_path = String::new();
                for j in 0..empty_depth {
                    if j > 0 {
                        empty_path.push('/');
                    }
                    empty_path.push_str(&format!("empty_dir{}", j));
                }
                entries.push(make_entry(&format!("{}/", empty_path), 0, true));
            }

            let pruned = RuleEngine::prune_entries(&entries);

            // Verify game files present (should be 38)
            let game_files = pruned
                .iter()
                .filter(|e| {
                    if game_prefix.is_empty() {
                        !e.path.contains("ImagePack")
                    } else {
                        e.path.starts_with(&game_prefix) && !e.path.contains("ImagePack")
                    }
                })
                .count();

            // Verify ImagePack files present (should be 3)
            let imagepack_files = pruned
                .iter()
                .filter(|e| e.path.contains("ImagePack"))
                .count();

            // Check no junk remains
            let has_junk = pruned
                .iter()
                .any(|e| e.path.contains("junk") || e.path.contains("empty_dir"));

            assert_eq!(
                game_files, 38,
                "Iteration {}: Expected 38 game files at depth {}, got {}",
                iteration, game_depth, game_files
            );
            assert_eq!(
                imagepack_files, 3,
                "Iteration {}: Expected 3 ImagePack files at depth {}, got {}",
                iteration, imagepack_depth, imagepack_files
            );
            assert!(
                !has_junk,
                "Iteration {}: Found junk in pruned results",
                iteration
            );

            // Total should be 41
            assert_eq!(
                pruned.len(),
                41,
                "Iteration {}: Game depth={}, ImagePack depth={}, expected 41 files, got {}",
                iteration,
                game_depth,
                imagepack_depth,
                pruned.len()
            );
        }
    }

    #[test]
    fn test_wave_random_extreme_patterns() {
        let _rng = rand::rng();

        // Test extreme scenarios
        for _ in 0..3 {
            let mut entries = vec![];

            // Extreme case 1: Game at max depth, ImagePack at root
            let game_at_depth = "a/b/c/d/e";
            entries.extend(make_japanese_game(game_at_depth));
            entries.push(make_entry("ImagePack/pic1.png", 256000, false));
            entries.push(make_entry("ImagePack/pic2.png", 128000, false));

            // Add massive junk noise
            for i in 0..20 {
                entries.push(make_entry(&format!("junk{}/", i), 0, true));
                entries.push(make_entry(&format!("empty{}.tmp", i), 0, false));
            }

            // Add junk at intermediate depths
            for i in 0..5 {
                entries.push(make_entry(&format!("a/b/junk{}/", i), 0, true));
                entries.push(make_entry(&format!("a/b/c/empty{}.log", i), 0, false));
            }

            let pruned = RuleEngine::prune_entries(&entries);

            // Should have exactly 40 files (38 game + 2 images)
            assert_eq!(pruned.len(), 40);
            assert!(pruned.iter().any(|e| e.path.starts_with(game_at_depth)));
            assert!(pruned.iter().any(|e| e.path.starts_with("ImagePack/")));
            assert!(!pruned
                .iter()
                .any(|e| e.path.contains("junk") || e.path.contains("empty")));
        }
    }

    #[test]
    fn test_wave_game_plus_image_pack_folder() {
        // Archive -> nested -> game + ImagePack folder
        let mut entries = make_japanese_game("nested");

        // Add image pack folder as sibling
        entries.push(make_entry("nested/ImagePack/", 0, true));
        entries.push(make_entry(
            "nested/ImagePack/wallpaper01.png",
            1024000,
            false,
        ));
        entries.push(make_entry(
            "nested/ImagePack/wallpaper02.png",
            512000,
            false,
        ));
        entries.push(make_entry("nested/ImagePack/readme.txt", 256, false));

        // Add junk
        entries.push(make_entry("nested/temp/", 0, true));
        entries.push(make_entry("junk.tmp", 0, false));

        let pruned = RuleEngine::prune_entries(&entries);

        // Game files present
        assert!(pruned
            .iter()
            .any(|e| e.path == "nested/聖女マグノリアのクエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "nested/audio/bgm.ogg"));

        // Image pack files present
        assert!(pruned
            .iter()
            .any(|e| e.path == "nested/ImagePack/wallpaper01.png"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "nested/ImagePack/wallpaper02.png"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "nested/ImagePack/readme.txt"));

        // Junk removed
        assert!(!pruned.iter().any(|e| e.path.contains("temp")));
        assert!(!pruned.iter().any(|e| e.path == "junk.tmp"));

        // 38 game files + 3 image pack files = 41
        assert_eq!(pruned.len(), 41);
    }

    #[test]
    fn test_wave_game_plus_image_pack_archive() {
        // Archive -> nested -> game + ImagePack.zip file
        let mut entries = make_japanese_game("wrapper");

        // Add image pack as archive file (sibling to game)
        entries.push(make_entry("wrapper/ImagePack.zip", 50000000, false));
        entries.push(make_entry("wrapper/Patches.zip", 5000000, false));

        // Add junk
        entries.push(make_entry("wrapper/empty_dir/", 0, true));
        entries.push(make_entry("zero.log", 0, false));

        let pruned = RuleEngine::prune_entries(&entries);

        // Game files
        assert!(pruned
            .iter()
            .any(|e| e.path == "wrapper/聖女マグノリアのクエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "wrapper/data/maps.json"));

        // Archive files
        assert!(pruned.iter().any(|e| e.path == "wrapper/ImagePack.zip"));
        assert!(pruned.iter().any(|e| e.path == "wrapper/Patches.zip"));

        // Junk removed
        assert!(!pruned.iter().any(|e| e.path.contains("empty_dir")));
        assert!(!pruned.iter().any(|e| e.path == "zero.log"));

        // 38 game files + 2 archives = 40
        assert_eq!(pruned.len(), 40);
    }

    #[test]
    fn test_needle_in_haystack_game_deep() {
        // Single game file buried in massive noise
        let mut entries = vec![];

        // Lots of junk at root
        for i in 0..10 {
            entries.push(make_entry(&format!("junk{}/", i), 0, true));
            entries.push(make_entry(&format!("empty{}.tmp", i), 0, false));
        }

        // Deep path with game
        entries.extend(make_japanese_game("a/b/c"));

        // More junk mixed in
        entries.push(make_entry("a/junk/", 0, true));
        entries.push(make_entry("a/b/empty/", 0, true));
        entries.push(make_entry("a/b/c/trash/", 0, true));

        let pruned = RuleEngine::prune_entries(&entries);

        // Only game files should remain
        assert!(pruned
            .iter()
            .any(|e| e.path == "a/b/c/聖女マグノリアのクエスト.exe"));
        assert!(pruned.iter().any(|e| e.path == "a/b/c/locales/ja.pak"));

        // All junk removed
        assert!(!pruned.iter().any(|e| e.path.contains("junk")));
        assert!(!pruned.iter().any(|e| e.path.contains("empty")));
        assert!(!pruned.iter().any(|e| e.path.contains("trash")));

        // Only the 38 game files
        assert_eq!(pruned.len(), 38);
    }

    #[test]
    fn test_multiple_games_different_depths() {
        // Two game copies at different depths + image pack
        let mut entries1 = make_japanese_game("Game1");
        let mut entries2 = make_japanese_game("deep/nested/Game2");

        let mut entries = vec![];
        entries.append(&mut entries1);
        entries.append(&mut entries2);

        // Image pack at another depth
        entries.push(make_entry("ImagePack/", 0, true));
        entries.push(make_entry("ImagePack/pic1.jpg", 256000, false));
        entries.push(make_entry("ImagePack/pic2.jpg", 128000, false));

        // Junk
        entries.push(make_entry("empty/", 0, true));
        entries.push(make_entry("zero.tmp", 0, false));

        let pruned = RuleEngine::prune_entries(&entries);

        // Both games present
        assert!(pruned
            .iter()
            .any(|e| e.path == "Game1/聖女マグノリアのクエスト.exe"));
        assert!(pruned
            .iter()
            .any(|e| e.path == "deep/nested/Game2/聖女マグノリアのクエスト.exe"));

        // Image pack present
        assert!(pruned.iter().any(|e| e.path == "ImagePack/pic1.jpg"));
        assert!(pruned.iter().any(|e| e.path == "ImagePack/pic2.jpg"));

        // Junk removed
        assert!(!pruned.iter().any(|e| e.path == "zero.tmp"));

        // 38 + 38 + 2 = 78
        assert_eq!(pruned.len(), 78);
    }
}
