// Types for Password Rules feature
#[derive(Clone, Debug)]
pub struct PasswordRule {
    pub name: String,
    pub pattern: String,
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct RegexTestResult {
    pub file_path: String,
    pub matched: bool,
}