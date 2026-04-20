//! High-level UI events. The main loop translates raw key presses and IPC
//! events into these before applying them to `AppState`.

use llm_relay_core::ipc::Event as IpcEvent;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Quit,
    NextTab,
    PrevTab,
    Up,
    Down,
    Enter,
    Esc,
    Char(char),
    Refresh,
    Ipc(IpcEvent),
    /// User pressed 'a' while on the Settings tab — toggle auto-launch.
    ToggleAutoLaunch,
}
