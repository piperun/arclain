use crate::features::password_management::dialogs::PasswordDialog;
use crate::shared::SharedState;
use std::path::PathBuf;

pub enum PasswordFeatureAction {
    None,
    PasswordUnlocked {
        path: PathBuf,
        password: String,
    },
}

pub struct PasswordFeature {
    pub password_dialog: PasswordDialog,
}

impl PasswordFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            password_dialog: PasswordDialog::default(),
        }
    }
}
