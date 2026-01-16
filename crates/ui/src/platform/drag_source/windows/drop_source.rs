use tracing::info;
use windows::core::{implement, HRESULT};
use windows::Win32::Foundation::{BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK};
use windows::Win32::System::Ole::DROPEFFECT;

/// Simple drop source that tracks drag state
#[implement(windows::Win32::System::Ole::IDropSource)]
pub struct DropSource;

impl windows::Win32::System::Ole::IDropSource_Impl for DropSource {
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> HRESULT {
        // internal throttling to prove it's alive without spamming
        // info!("[drag] QueryContinueDrag: keys={:?}", grfkeystate);

        if fescapepressed.as_bool() {
            info!("[drag] QueryContinueDrag: Escape pressed, cancelling");
            DRAGDROP_S_CANCEL
        } else if (grfkeystate.0 & windows::Win32::System::SystemServices::MK_LBUTTON.0) == 0 {
            info!("[drag] QueryContinueDrag: LButton released, dropping");
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        // info!("[drag] GiveFeedback: effect={:?}", dweffect);
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
