//! Distro discovery + probe + SQLite cache. See parent module for overview.

#[derive(Debug, Clone)]
pub struct DistroRow {
    pub name: String,
    pub is_default: bool,
    pub selected: bool,
    pub home: Option<String>,
    pub user: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
    pub resolved_url: Option<String>,
    pub probed_at: Option<String>,
}
