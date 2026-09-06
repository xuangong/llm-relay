use crate::app::event::AppEvent;
use llm_relay_core::ipc::{KeyInfo, ModelCatalog, ModelSelection};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum Modal {
    AddGateway(AddGatewayForm),
    EditGateway(EditGatewayForm),
    Login(LoginForm),
    SelectKeyModel(SelectKeyModelForm),
}

#[derive(Debug, Clone, Default)]
pub struct AddGatewayForm {
    pub name: String,
    pub url: String,
    pub focus: AddField,
    pub error: Option<String>,
    pub submitting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddField {
    Name,
    Url,
}

impl Default for AddField {
    fn default() -> Self {
        AddField::Name
    }
}

#[derive(Debug, Clone)]
pub struct EditGatewayForm {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub focus: AddField,
    pub error: Option<String>,
}

impl Default for EditGatewayForm {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            name: String::new(),
            url: String::new(),
            focus: AddField::default(),
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoginForm {
    pub gateway_id: Uuid,
    pub gateway_name: String,
    pub state: LoginUiState,
}

#[derive(Debug, Clone)]
pub enum LoginUiState {
    Initiating,
    WaitingForUser {
        user_code: String,
        verification_uri: String,
        expires_in_secs: u64,
    },
    Completed,
    Failed(String),
    Expired,
}

#[derive(Debug, Clone)]
pub struct SelectKeyModelForm {
    pub gateway_id: Uuid,
    pub gateway_name: String,
    pub keys: Vec<KeyInfo>,
    pub selected_key_idx: usize,
    pub catalog: Option<ModelCatalog>,
    /// Indexes into the per-category model lists from `catalog`.
    pub claude_idx: usize,
    pub claude_subagent_idx: usize,
    pub claude_haiku_idx: usize,
    pub codex_idx: usize,
    pub codex_subagent_idx: usize,
    pub gemini_idx: usize,
    pub focus: SelectField,
    pub error: Option<String>,
    pub submitting: bool,
    pub loading_models: bool,
}

impl SelectKeyModelForm {
    /// Currently selected key, if any.
    pub fn selected_key(&self) -> Option<&KeyInfo> {
        self.keys.get(self.selected_key_idx)
    }

    /// Build a `ModelSelection` from the current indexes.
    pub fn model_selection(&self) -> ModelSelection {
        let cat = match &self.catalog {
            Some(c) => c,
            None => return ModelSelection::default(),
        };
        ModelSelection {
            claude: cat.claude.get(self.claude_idx).cloned(),
            claude_subagent: cat.claude.get(self.claude_subagent_idx).cloned(),
            claude_small: cat.claude.get(self.claude_haiku_idx).cloned(),
            codex: cat.codex.get(self.codex_idx).cloned(),
            codex_subagent: cat.codex.get(self.codex_subagent_idx).cloned(),
            gemini: cat.gemini.get(self.gemini_idx).cloned(),
            claude_extra: llm_relay_core::ipc::ClaudeExtraSelection::Inherit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectField {
    Key,
    Claude,
    ClaudeSubagent,
    ClaudeHaiku,
    Codex,
    CodexSubagent,
    Gemini,
}

impl SelectField {
    pub fn next(self) -> Self {
        match self {
            Self::Key => Self::Claude,
            Self::Claude => Self::ClaudeSubagent,
            Self::ClaudeSubagent => Self::ClaudeHaiku,
            Self::ClaudeHaiku => Self::Codex,
            Self::Codex => Self::CodexSubagent,
            Self::CodexSubagent => Self::Gemini,
            Self::Gemini => Self::Key,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Self::Key => Self::Gemini,
            Self::Claude => Self::Key,
            Self::ClaudeSubagent => Self::Claude,
            Self::ClaudeHaiku => Self::ClaudeSubagent,
            Self::Codex => Self::ClaudeHaiku,
            Self::CodexSubagent => Self::Codex,
            Self::Gemini => Self::CodexSubagent,
        }
    }
}

/// Routing decision: handled here vs. fall through.
#[derive(Debug)]
pub enum ModalOutcome {
    Consumed,
    PassThrough,
    Submit(ModalSubmit),
    Close,
}

#[derive(Debug)]
pub enum ModalSubmit {
    AddGateway {
        name: String,
        url: String,
    },
    EditGateway {
        id: Uuid,
        name: String,
        url: String,
    },
    SaveConfig {
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    },
}

impl Modal {
    /// Apply a key/edit event to the active modal. Returns whether to
    /// consume, pass through, submit, or close.
    pub fn handle(&mut self, event: &AppEvent) -> ModalOutcome {
        match self {
            Modal::AddGateway(f) => add_handle(f, event),
            Modal::EditGateway(f) => edit_handle(f, event),
            Modal::Login(_) => login_handle(event),
            Modal::SelectKeyModel(f) => select_key_model_handle(f, event),
        }
    }
}

fn add_handle(f: &mut AddGatewayForm, event: &AppEvent) -> ModalOutcome {
    if f.submitting {
        return ModalOutcome::Consumed;
    }
    match event {
        AppEvent::Esc => ModalOutcome::Close,
        AppEvent::Enter => {
            if f.name.trim().is_empty() {
                f.error = Some("Name is required".into());
                return ModalOutcome::Consumed;
            }
            if !f.url.starts_with("http://") && !f.url.starts_with("https://") {
                f.error = Some("URL must start with http:// or https://".into());
                return ModalOutcome::Consumed;
            }
            ModalOutcome::Submit(ModalSubmit::AddGateway {
                name: f.name.clone(),
                url: f.url.clone(),
            })
        }
        AppEvent::Char(c) => {
            target_buf(f).push(*c);
            ModalOutcome::Consumed
        }
        AppEvent::Up | AppEvent::Down => {
            f.focus = match f.focus {
                AddField::Name => AddField::Url,
                AddField::Url => AddField::Name,
            };
            ModalOutcome::Consumed
        }
        _ => ModalOutcome::Consumed,
    }
}

fn target_buf(f: &mut AddGatewayForm) -> &mut String {
    match f.focus {
        AddField::Name => &mut f.name,
        AddField::Url => &mut f.url,
    }
}

fn edit_handle(f: &mut EditGatewayForm, event: &AppEvent) -> ModalOutcome {
    match event {
        AppEvent::Esc => ModalOutcome::Close,
        AppEvent::Enter => {
            if f.name.trim().is_empty() {
                f.error = Some("Name is required".into());
                return ModalOutcome::Consumed;
            }
            if !f.url.starts_with("http://") && !f.url.starts_with("https://") {
                f.error = Some("URL must start with http:// or https://".into());
                return ModalOutcome::Consumed;
            }
            ModalOutcome::Submit(ModalSubmit::EditGateway {
                id: f.id,
                name: f.name.clone(),
                url: f.url.clone(),
            })
        }
        AppEvent::Char(c) => {
            match f.focus {
                AddField::Name => f.name.push(*c),
                AddField::Url => f.url.push(*c),
            }
            ModalOutcome::Consumed
        }
        AppEvent::Up | AppEvent::Down => {
            f.focus = if f.focus == AddField::Name {
                AddField::Url
            } else {
                AddField::Name
            };
            ModalOutcome::Consumed
        }
        _ => ModalOutcome::Consumed,
    }
}

fn login_handle(event: &AppEvent) -> ModalOutcome {
    match event {
        AppEvent::Esc => ModalOutcome::Close,
        AppEvent::Char('c') => ModalOutcome::Consumed, // copy handled in loop_ — needs IO
        AppEvent::Ipc(_) => ModalOutcome::PassThrough, // let IPC events reach state.apply_ipc
        _ => ModalOutcome::Consumed,
    }
}

fn select_key_model_handle(f: &mut SelectKeyModelForm, event: &AppEvent) -> ModalOutcome {
    if f.submitting || f.loading_models {
        return ModalOutcome::Consumed;
    }
    match event {
        AppEvent::Esc => ModalOutcome::Close,
        AppEvent::Up => {
            f.focus = f.focus.prev();
            ModalOutcome::Consumed
        }
        AppEvent::Down => {
            f.focus = f.focus.next();
            ModalOutcome::Consumed
        }
        AppEvent::Left => {
            select_prev(f);
            ModalOutcome::Consumed
        }
        AppEvent::Right => {
            select_next(f);
            ModalOutcome::Consumed
        }
        AppEvent::Enter => {
            let key = match f.selected_key() {
                Some(k) => k,
                None => {
                    f.error = Some("No key available".into());
                    return ModalOutcome::Consumed;
                }
            };
            ModalOutcome::Submit(ModalSubmit::SaveConfig {
                gateway_id: f.gateway_id,
                key_id: key.id,
                models: f.model_selection(),
            })
        }
        AppEvent::Ipc(_) => ModalOutcome::PassThrough,
        _ => ModalOutcome::Consumed,
    }
}

fn select_prev(f: &mut SelectKeyModelForm) {
    match f.focus {
        SelectField::Key => {
            if f.selected_key_idx > 0 {
                f.selected_key_idx -= 1;
            }
        }
        SelectField::Claude => {
            if f.claude_idx > 0 {
                f.claude_idx -= 1;
            }
        }
        SelectField::ClaudeSubagent => {
            if f.claude_subagent_idx > 0 {
                f.claude_subagent_idx -= 1;
            }
        }
        SelectField::ClaudeHaiku => {
            if f.claude_haiku_idx > 0 {
                f.claude_haiku_idx -= 1;
            }
        }
        SelectField::Codex => {
            if f.codex_idx > 0 {
                f.codex_idx -= 1;
            }
        }
        SelectField::CodexSubagent => {
            if f.codex_subagent_idx > 0 {
                f.codex_subagent_idx -= 1;
            }
        }
        SelectField::Gemini => {
            if f.gemini_idx > 0 {
                f.gemini_idx -= 1;
            }
        }
    }
}

fn select_next(f: &mut SelectKeyModelForm) {
    let cat = f.catalog.as_ref();
    match f.focus {
        SelectField::Key => {
            if f.selected_key_idx + 1 < f.keys.len() {
                f.selected_key_idx += 1;
            }
        }
        SelectField::Claude => {
            let max = cat.map(|c| c.claude.len()).unwrap_or(0);
            if f.claude_idx + 1 < max {
                f.claude_idx += 1;
            }
        }
        SelectField::ClaudeSubagent => {
            let max = cat.map(|c| c.claude.len()).unwrap_or(0);
            if f.claude_subagent_idx + 1 < max {
                f.claude_subagent_idx += 1;
            }
        }
        SelectField::ClaudeHaiku => {
            let max = cat.map(|c| c.claude.len()).unwrap_or(0);
            if f.claude_haiku_idx + 1 < max {
                f.claude_haiku_idx += 1;
            }
        }
        SelectField::Codex => {
            let max = cat.map(|c| c.codex.len()).unwrap_or(0);
            if f.codex_idx + 1 < max {
                f.codex_idx += 1;
            }
        }
        SelectField::CodexSubagent => {
            let max = cat.map(|c| c.codex.len()).unwrap_or(0);
            if f.codex_subagent_idx + 1 < max {
                f.codex_subagent_idx += 1;
            }
        }
        SelectField::Gemini => {
            let max = cat.map(|c| c.gemini.len()).unwrap_or(0);
            if f.gemini_idx + 1 < max {
                f.gemini_idx += 1;
            }
        }
    }
}
