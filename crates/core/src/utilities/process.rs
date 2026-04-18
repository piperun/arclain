//! Process spawn helpers.
//!
//! On Windows, suppress the console window that normally pops up when
//! spawning a subprocess (e.g. 7z.exe, unrar.exe) from a GUI app. Without
//! this the user sees a flashing cmd window for every backend operation.

use std::process::Command;

/// Apply platform-specific flags to hide the console window for GUI apps.
/// On non-Windows platforms this is a no-op.
pub fn hide_console(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
