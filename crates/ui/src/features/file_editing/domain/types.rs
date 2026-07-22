#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FileEditLoadState {
    #[default]
    Idle,
    Loading {
        request_id: u64,
    },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileEditDialog {
    pub show: bool,
    pub full_path_in_archive: String,
    pub name_input: String,
    pub content: String,
    pub original_content: String,
    pub error: String,
    pub load_state: FileEditLoadState,
}

impl FileEditDialog {
    /// Apply state returned by an immediate-mode render pass without allowing
    /// an older snapshot to replace a newer worker completion or request.
    pub fn apply_rendered_snapshot(&mut self, rendered: Self) {
        if self.full_path_in_archive != rendered.full_path_in_archive {
            return;
        }

        if self.load_state == rendered.load_state {
            *self = rendered;
        } else if !rendered.show {
            // A cancel/close action for this same path still takes effect, but
            // preserves content and load state published during the frame.
            self.show = false;
        }
    }
}

pub enum FileEditResult {
    Save { new_name: String, content: String },
    Cancel,
}
