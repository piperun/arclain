// Platform-specific utilities

#[cfg(target_os = "windows")]
pub fn detect_dark_mode() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) =
        hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
    {
        if let Ok(value) = key.get_value::<u32, _>("AppsUseLightTheme") {
            return value == 0;
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub fn detect_dark_mode() -> bool {
    false
}