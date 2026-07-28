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

/// The durable notice shown (via `error`) when a save was submitted with
/// a changed name -- see [`FileEditDialog::apply_save_outcome`].
pub const RENAME_NOT_HONORED_NOTICE: &str =
    "Renaming while saving is not supported yet -- content was saved under the original name.";

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

    /// Applies the outcome of a `Save` click. `ArchiveMutationRequest::
    /// ReplaceText` (what a save actually submits) has no rename/move
    /// concept -- content always lands back at this dialog's own
    /// `full_path_in_archive`, never at `new_name` if that differs. When
    /// it does differ, this keeps the dialog open with a durable notice
    /// in `error` instead of closing it: the bridge writes its own
    /// "Archive updated" status-bar message once the save actually
    /// completes, on a background task racing whatever render pass calls
    /// this -- a transient status-bar note here could be silently
    /// overwritten by that before the user ever saw it, but `error` lives
    /// on this dialog's own signal, untouched by that completion event.
    /// When the name is unchanged, closes the dialog as usual.
    pub fn apply_save_outcome(&mut self, new_name: &str) {
        if new_name != self.full_path_in_archive {
            self.error = RENAME_NOT_HONORED_NOTICE.to_string();
        } else {
            self.show = false;
        }
    }
}

pub enum FileEditResult {
    Save { new_name: String, content: String },
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_dialog(path: &str) -> FileEditDialog {
        FileEditDialog {
            show: true,
            full_path_in_archive: path.to_string(),
            name_input: path.to_string(),
            content: "content".to_string(),
            original_content: "content".to_string(),
            error: String::new(),
            load_state: FileEditLoadState::Ready,
        }
    }

    #[test]
    fn an_unchanged_name_closes_the_dialog_without_a_notice() {
        let mut dialog = ready_dialog("readme.txt");

        dialog.apply_save_outcome("readme.txt");

        assert!(!dialog.show);
        assert!(dialog.error.is_empty());
    }

    /// The regression this method exists to fix: pre-fix, the rename
    /// notice was a transient status-bar write from the same
    /// fire-and-forget task that submitted the mutation, racing (and
    /// routinely losing to) the bridge's own later "Archive updated"
    /// status-bar write once the save actually completed. `error` lives
    /// on this dialog's own field -- a completely different signal from
    /// the status bar -- and keeping `show` true means the dialog stays
    /// on screen displaying it, so there is no write for anything else to
    /// race against, let alone lose to.
    #[test]
    fn a_changed_name_keeps_the_dialog_open_with_a_durable_notice() {
        let mut dialog = ready_dialog("readme.txt");

        dialog.apply_save_outcome("renamed.txt");

        assert!(
            dialog.show,
            "the dialog must stay open so the notice is actually seen"
        );
        assert_eq!(dialog.error, RENAME_NOT_HONORED_NOTICE);
    }
}
