// use crate::features::password_management::dialogs::PasswordDialog;
use std::path::PathBuf;

pub enum PasswordFeatureAction {
    None,
    PasswordUnlocked { path: PathBuf, password: String },
}
