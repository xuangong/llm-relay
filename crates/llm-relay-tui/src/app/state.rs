//! Pure state. No I/O, no ratatui, no crossterm.
//! Everything that mutates state goes through `handle(AppEvent)` so we can
//! unit-test behavior without a terminal.

use crate::app::event::AppEvent;
use llm_relay_core::ipc::{Event as IpcEvent, HealthStatus, UsageRange, UsageRowDetail};
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
}

#[derive(Debug, Default)]
pub struct UsageState {
    pub range: UsageRange,
    pub rows: Vec<UsageRowDetail>,
    pub selected: usize,
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
