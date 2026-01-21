#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileEditDialog {
    pub show: bool,
    pub full_path_in_archive: String,
    pub name_input: String,
    pub content: String,
    pub original_content: String,
    pub error: String,
}

pub enum FileEditResult {
    Save { new_name: String, content: String },
    Cancel,
}
