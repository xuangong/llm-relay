//! Pure state. No I/O, no ratatui, no crossterm.
//! Everything that mutates state goes through `handle(AppEvent)` so we can
//! unit-test behavior without a terminal.

use crate::app::event::AppEvent;
use llm_relay_core::ipc::{ErrorRow, Event as IpcEvent, HealthStatus, TuiSettings, UsageRange, UsageRowDetail};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Gateways,
    Usage,
    Errors,
    Settings,
}

impl Tab {
    fn next(self) -> Self {
        match self {
            Tab::Gateways => Tab::Usage,
            Tab::Usage => Tab::Errors,
            Tab::Errors => Tab::Settings,
            Tab::Settings => Tab::Gateways,
        }
    }
    fn prev(self) -> Self {
        match self {
            Tab::Gateways => Tab::Settings,
            Tab::Usage => Tab::Gateways,
            Tab::Errors => Tab::Usage,
            Tab::Settings => Tab::Errors,
        }
    }
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Gateways
    }
}

#[derive(Debug, Clone, Default)]
pub struct GatewayRow {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub healthy: Option<bool>,
    pub latency_ms: Option<i64>,
    pub starred: bool,
    pub expanded: bool,
    /// Mirrors `GatewaySummary::needs_login` — render a 🔒 in the gateway list.
    pub needs_login: bool,
}

#[derive(Debug, Default)]
pub struct UsageState {
    pub range: UsageRange,
    pub rows: Vec<UsageRowDetail>,
    pub selected: usize,
    /// If the most recent fetch returned an error (e.g. NotImplemented), the
    /// view renders a banner instead of an empty table.
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ErrorsState {
    pub rows: Vec<ErrorRow>,
    pub selected: usize,
    /// Same purpose as `UsageState::error` — surface fetch errors to the UI.
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub snapshot: Option<TuiSettings>,
}

#[derive(Debug, Default)]
pub struct AppState {
    pub active_tab: Tab,
    pub should_quit: bool,
    pub gateways: Vec<GatewayRow>,
    pub gateway_index: HashMap<Uuid, usize>,
    pub selected_row: usize,
    pub status_message: Option<String>,
    pub usage: UsageState,
    pub errors: ErrorsState,
    pub settings: SettingsState,
    pub modal: Option<crate::app::modal::Modal>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => self.should_quit = true,
            AppEvent::NextTab => self.active_tab = self.active_tab.next(),
            AppEvent::PrevTab => self.active_tab = self.active_tab.prev(),
            AppEvent::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            AppEvent::Down => {
                if self.selected_row + 1 < self.gateways.len() {
                    self.selected_row += 1;
                }
            }
            AppEvent::Enter => {
                if let Some(row) = self.gateways.get_mut(self.selected_row) {
                    row.expanded = !row.expanded;
                }
            }
            AppEvent::Esc => { /* dialogs handle this; default no-op */ }
            AppEvent::Char(c) => match c {
                's' => {
                    if let Some(row) = self.gateways.get_mut(self.selected_row) {
                        row.starred = !row.starred;
                    }
                }
                'p' if self.active_tab == Tab::Usage => self.cycle_usage_range(),
                _ => {}
            },
            AppEvent::Refresh => { /* triggers an IPC fetch in the loop */ }
            AppEvent::Ipc(evt) => self.apply_ipc(evt),
            // ToggleAutoLaunch is handled asynchronously in loop_.rs after emitting the IPC call.
            // The state update (flipping auto_launch in snapshot) happens after re-fetching settings.
            AppEvent::ToggleAutoLaunch => { /* handled by loop_.rs */ }
        }
    }

    fn apply_ipc(&mut self, evt: IpcEvent) {
        match evt {
            IpcEvent::HealthChanged { gateway_id, status } => {
                if let Some(&idx) = self.gateway_index.get(&gateway_id) {
                    if let Some(row) = self.gateways.get_mut(idx) {
                        row.healthy = Some(matches!(status, HealthStatus::Healthy));
                    }
                }
            }
            IpcEvent::LoginCompleted { gateway_id, .. } => {
                use crate::app::modal::LoginUiState;
                if let Some(crate::app::modal::Modal::Login(f)) = self.modal.as_mut() {
                    if f.gateway_id == gateway_id {
                        f.state = LoginUiState::Completed;
                    }
                }
                // Clear needs_login flag on the gateway row.
                if let Some(&idx) = self.gateway_index.get(&gateway_id) {
                    if let Some(row) = self.gateways.get_mut(idx) {
                        row.needs_login = false;
                    }
                }
            }
            IpcEvent::LoginFailed { gateway_id, message } => {
                use crate::app::modal::LoginUiState;
                if let Some(crate::app::modal::Modal::Login(f)) = self.modal.as_mut() {
                    if f.gateway_id == gateway_id {
                        f.state = LoginUiState::Failed(message);
                    }
                }
            }
            IpcEvent::LoginExpired { gateway_id } => {
                use crate::app::modal::LoginUiState;
                if let Some(crate::app::modal::Modal::Login(f)) = self.modal.as_mut() {
                    if f.gateway_id == gateway_id {
                        f.state = LoginUiState::Expired;
                    }
                }
            }
            // Other event variants handled in later phases.
            _ => {}
        }
    }

    pub fn replace_gateways(&mut self, rows: Vec<GatewayRow>) {
        self.gateway_index.clear();
        for (i, row) in rows.iter().enumerate() {
            self.gateway_index.insert(row.id, i);
        }
        self.gateways = rows;
        if self.selected_row >= self.gateways.len() {
            self.selected_row = self.gateways.len().saturating_sub(1);
        }
    }

    /// Cycle the usage range filter: Today → Last7Days → Last30Days → AllTime → Today.
    pub fn cycle_usage_range(&mut self) {
        self.usage.range = match self.usage.range {
            UsageRange::Today => UsageRange::Last7Days,
            UsageRange::Last7Days => UsageRange::Last30Days,
            UsageRange::Last30Days => UsageRange::AllTime,
            UsageRange::AllTime => UsageRange::Today,
        };
    }
}
