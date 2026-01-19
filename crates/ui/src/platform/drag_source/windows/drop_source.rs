//! Drop source implementation for drag-and-drop operations.
//!
//! This implementation coordinates with HDropDataObject to implement
//! 7-Zip style deferred extraction - extraction only happens on actual drop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;
use windows::core::{implement, HRESULT};
use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, POINT, S_OK,
};
use windows::Win32::System::Ole::DROPEFFECT;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, WindowFromPoint,
};

/// Check if the cursor is currently over our own application window.
/// Used to cancel drops over arclain and show "NO" cursor.
fn is_cursor_over_own_window() -> bool {
    unsafe {
        // Get current cursor position
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_err() {
            return false;
        }

        // Get window under cursor
        let hwnd_under_cursor = WindowFromPoint(point);
        if hwnd_under_cursor.0 == 0 {
            return false;
        }

        // Get our foreground window (arclain main window)
        let our_hwnd = GetForegroundWindow();
        if our_hwnd.0 == 0 {
            return false;
        }

        // Compare by process ID - same process means same app
        let mut our_pid = 0u32;
        let mut cursor_pid = 0u32;
        GetWindowThreadProcessId(our_hwnd, Some(&mut our_pid));
        GetWindowThreadProcessId(hwnd_under_cursor, Some(&mut cursor_pid));

        our_pid != 0 && our_pid == cursor_pid
    }
}

/// Shared state between DropSource and HDropDataObject.
///
/// This allows QueryContinueDrag to signal the data object when
/// a drop occurs, triggering deferred extraction.
#[derive(Default)]
pub struct DragState {
    /// When true, return pre-extraction HDROP (just temp folder).
    /// Set to false when drop is confirmed.
    pub use_pre_global: AtomicBool,

    /// When true, extraction should happen on next GetData call.
    pub need_extract: AtomicBool,

    /// When true, extraction has completed.
    pub extract_done: AtomicBool,

    /// Last effect returned by GiveFeedback (used to detect if drop is allowed).
    pub last_effect: std::sync::atomic::AtomicU32,
}

impl DragState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            use_pre_global: AtomicBool::new(true),
            need_extract: AtomicBool::new(false),
            extract_done: AtomicBool::new(false),
            last_effect: std::sync::atomic::AtomicU32::new(0),
        })
    }
}

/// Drop source that coordinates with HDropDataObject for deferred extraction.
///
/// When the mouse button is released (drop), this sets flags in DragState
/// to trigger extraction in the data object.
#[implement(windows::Win32::System::Ole::IDropSource)]
pub struct DropSourceWithState {
    state: Arc<DragState>,
}

impl DropSourceWithState {
    pub fn new(state: Arc<DragState>) -> Self {
        Self { state }
    }
}

impl windows::Win32::System::Ole::IDropSource_Impl for DropSourceWithState {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            info!("[drag] QueryContinueDrag: Escape pressed, cancelling");
            return DRAGDROP_S_CANCEL;
        }

        if (grfkeystate.0 & MK_LBUTTON.0) == 0 {
            // Mouse button released = potential DROP

            // Check if dropping over our own window - if so, cancel
            // This prevents wasted extraction and shows "NO" cursor
            if is_cursor_over_own_window() {
                info!("[drag] QueryContinueDrag: Drop over own window, cancelling");
                return DRAGDROP_S_CANCEL;
            }

            let effect = self.state.last_effect.load(Ordering::SeqCst);

            if effect == 0 {
                // DROPEFFECT_NONE - target doesn't accept drop
                info!("[drag] QueryContinueDrag: Drop not allowed (effect=0), cancelling");
                return DRAGDROP_S_CANCEL;
            }

            info!("[drag] QueryContinueDrag: LButton released, triggering drop");

            // Switch to final HDROP and trigger extraction
            self.state.use_pre_global.store(false, Ordering::SeqCst);
            self.state.need_extract.store(true, Ordering::SeqCst);

            return DRAGDROP_S_DROP;
        }

        S_OK
    }

    fn GiveFeedback(&self, dweffect: DROPEFFECT) -> HRESULT {
        // Store the effect so QueryContinueDrag knows if drop is allowed
        self.state.last_effect.store(dweffect.0, Ordering::SeqCst);

        // Show NO cursor when hovering over our own window
        if is_cursor_over_own_window() {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_NO};
                let no_cursor = LoadCursorW(None, IDC_NO).unwrap_or_default();
                SetCursor(no_cursor);
            }
            // Return S_OK to indicate we handled the cursor
            return S_OK;
        }

        // Let Windows handle the default cursors
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

// Keep the old simple DropSource for backwards compatibility
/// Simple drop source that tracks drag state (legacy, no deferred extraction)
#[implement(windows::Win32::System::Ole::IDropSource)]
pub struct DropSource;

impl windows::Win32::System::Ole::IDropSource_Impl for DropSource {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            info!("[drag] QueryContinueDrag: Escape pressed, cancelling");
            DRAGDROP_S_CANCEL
        } else if (grfkeystate.0 & MK_LBUTTON.0) == 0 {
            info!("[drag] QueryContinueDrag: LButton released, dropping");
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
