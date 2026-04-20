use crate::app::event::AppEvent;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum Modal {
    AddGateway(AddGatewayForm),
    EditGateway(EditGatewayForm),
    Login(LoginForm),
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
    AddGateway { name: String, url: String },
    EditGateway { id: Uuid, name: String, url: String },
}

impl Modal {
    /// Apply a key/edit event to the active modal. Returns whether to
    /// consume, pass through, submit, or close.
    pub fn handle(&mut self, event: &AppEvent) -> ModalOutcome {
        match self {
            Modal::AddGateway(f) => add_handle(f, event),
            Modal::EditGateway(f) => edit_handle(f, event),
            Modal::Login(_) => login_handle(event),
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
        _ => ModalOutcome::Consumed,
    }
}
