# LLM Relay TUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a cross-platform TUI version of LLM Relay (gateway management, health monitoring, traffic monitoring, OAuth device login) that can run on Linux servers without a desktop environment, sharing core logic with the existing Tauri GUI.

**Architecture:** Restructure the project into a cargo workspace with three crates: `llm-relay-core` (UI-agnostic logic — DB, gateway, health, proxy, config_writer, keystore, IPC protocol), `llm-relay-agent` (a long-running daemon binary with proxy + health loop + IPC server, fork+detach managed), and `llm-relay-tui` (ratatui frontend that connects to the agent over a Unix socket / Windows named pipe). The existing Tauri GUI is rewired to depend on `llm-relay-core` while keeping its in-process embedded proxy. GUI and TUI agent are mutually exclusive on port 18080 + a cross-platform file lock.

**Tech Stack:** Rust 2021, ratatui + crossterm (TUI), interprocess (cross-platform local socket), fs2 (file lock), daemonize (Unix detach), keyring + AES-256-GCM/argon2 (secrets), serde_json (IPC codec), tokio (async runtime).

**Spec reference:** `docs/superpowers/specs/2026-04-20-tui-design.md`

**Repository layout after this plan:**

```
llm-relay/
├── Cargo.toml                       # NEW — workspace root
├── crates/
│   ├── llm-relay-core/              # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs             # moved from src-tauri/src/error.rs
│   │       ├── database.rs          # moved
│   │       ├── gateway.rs           # moved
│   │       ├── health.rs            # moved (event-emission abstracted)
│   │       ├── proxy_server.rs      # moved (event-emission abstracted)
│   │       ├── config_writer.rs     # moved
│   │       ├── keystore.rs          # extended with EncryptedFile backend
│   │       ├── events.rs            # NEW — EventSink trait
│   │       ├── service.rs           # NEW — high-level façade used by GUI & agent
│   │       └── ipc/                 # NEW
│   │           ├── mod.rs
│   │           ├── protocol.rs      # Request / Response / Event / envelopes
│   │           └── codec.rs         # length-prefix framing
│   ├── llm-relay-agent/             # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── lifecycle.rs         # lock + pidfile + sock cleanup
│   │       ├── ipc_server.rs        # accept loop + per-conn router
│   │       └── login_manager.rs     # device-code state machine
│   └── llm-relay-tui/               # NEW
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs              # arg parsing + spawn/attach
│           ├── spawn.rs             # cross-platform detached agent spawn
│           ├── ipc_client.rs        # IPC client + event broadcaster
│           ├── app.rs               # AppState + tab routing
│           ├── views/
│           │   ├── mod.rs
│           │   ├── gateways.rs
│           │   ├── usage.rs
│           │   ├── errors.rs
│           │   └── settings.rs
│           ├── widgets/
│           │   ├── mod.rs
│           │   ├── add_gateway.rs
│           │   ├── login_dialog.rs
│           │   ├── key_picker.rs
│           │   └── model_picker.rs
│           └── theme.rs
├── src-tauri/                       # MODIFIED — depends on llm-relay-core
│   └── src/{main.rs,lib.rs,commands.rs,tray.rs}   # only Tauri-specific code remains
├── docs/
│   └── systemd/llm-relay-agent.service.example    # NEW
└── docs/superpowers/{specs,plans,reviews}/
```

**Implementation phases (this single plan, executed top-to-bottom):**

1. **Workspace + Core extraction** — Tasks 1–7
2. **Keystore fallback** — Tasks 8–10
3. **IPC protocol & codec** — Tasks 11–14
4. **Service façade & EventSink** — Tasks 15–17
5. **Agent binary (lifecycle + IPC server)** — Tasks 18–22
6. **Login state machine on agent** — Task 23
7. **TUI: spawn/attach + IPC client** — Tasks 24–26
8. **TUI: app shell + Gateways tab** — Tasks 27–29
9. **TUI: Usage / Errors / Settings tabs** — Tasks 30–32
10. **TUI: Add/Edit/Login dialogs** — Tasks 33–35
11. **Cross-platform spawn polish + lifecycle edge cases** — Tasks 36–38
12. **Docs (systemd, README) + CI smoke** — Tasks 39–40

---

## Phase 1: Workspace + Core extraction

The goal of this phase is mechanical, low-risk: move existing `src-tauri/src/*.rs` files (other than Tauri-specific bits) into a new `llm-relay-core` crate without changing behavior. The Tauri app continues to depend on the same code through the new crate. We do this BEFORE writing any new functionality so that subsequent phases don't have to keep editing the same files in two locations.

### Task 1: Create cargo workspace skeleton

**Files:**
- Create: `Cargo.toml` (workspace root)
- Modify: `src-tauri/Cargo.toml` (add `workspace = true` markers for shared deps later — for now just verify it still builds)

- [ ] **Step 1: Write workspace Cargo.toml**

Create `/Users/zhangxian/projects/llm-relay/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
    "src-tauri",
    "crates/llm-relay-core",
    "crates/llm-relay-agent",
    "crates/llm-relay-tui",
]

[workspace.package]
version = "0.3.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
reqwest = { version = "0.12", features = ["json", "stream"] }
axum = "0.8"
futures-util = "0.3"
rusqlite = { version = "0.32", features = ["bundled"] }
toml_edit = "0.22"
dirs = "5"
hostname = "0.4"
log = "0.4"
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }
anyhow = "1"
thiserror = "1"
```

- [ ] **Step 2: Create empty crate directories with placeholder Cargo.toml + lib/main**

```bash
mkdir -p crates/llm-relay-core/src crates/llm-relay-agent/src crates/llm-relay-tui/src
```

Create `crates/llm-relay-core/Cargo.toml`:
```toml
[package]
name = "llm-relay-core"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
reqwest.workspace = true
axum.workspace = true
futures-util.workspace = true
rusqlite.workspace = true
toml_edit.workspace = true
dirs.workspace = true
hostname.workspace = true
log.workspace = true
keyring.workspace = true
thiserror.workspace = true
```

Create `crates/llm-relay-core/src/lib.rs`:
```rust
//! Shared core for LLM Relay (UI-agnostic).
```

Create `crates/llm-relay-agent/Cargo.toml`:
```toml
[package]
name = "llm-relay-agent"
version.workspace = true
edition.workspace = true

[[bin]]
name = "llm-relay-agent"
path = "src/main.rs"

[dependencies]
llm-relay-core = { path = "../llm-relay-core" }
tokio.workspace = true
serde_json.workspace = true
log.workspace = true
anyhow.workspace = true
```

Create `crates/llm-relay-agent/src/main.rs`:
```rust
fn main() {
    println!("llm-relay-agent placeholder");
}
```

Create `crates/llm-relay-tui/Cargo.toml`:
```toml
[package]
name = "llm-relay-tui"
version.workspace = true
edition.workspace = true

[[bin]]
name = "llm-relay-tui"
path = "src/main.rs"

[dependencies]
llm-relay-core = { path = "../llm-relay-core" }
tokio.workspace = true
serde_json.workspace = true
log.workspace = true
anyhow.workspace = true
```

Create `crates/llm-relay-tui/src/main.rs`:
```rust
fn main() {
    println!("llm-relay-tui placeholder");
}
```

- [ ] **Step 3: Verify workspace builds**

Run from repo root: `cargo build --workspace`
Expected: All four crates compile successfully (warnings about unused deps are OK).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore: scaffold cargo workspace with core/agent/tui crates"
```

### Task 2: Move `error.rs` into core

**Files:**
- Create: `crates/llm-relay-core/src/error.rs`
- Delete: `src-tauri/src/error.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`
- Modify: `src-tauri/src/lib.rs` (re-export from core)
- Modify: `src-tauri/Cargo.toml` (add core dependency, drop now-unused deps later)

- [ ] **Step 1: Move file**

```bash
git mv src-tauri/src/error.rs crates/llm-relay-core/src/error.rs
```

- [ ] **Step 2: Re-export from core lib.rs**

Edit `crates/llm-relay-core/src/lib.rs`:
```rust
//! Shared core for LLM Relay (UI-agnostic).
pub mod error;
pub use error::AppError;
```

- [ ] **Step 3: Add core as a dependency in src-tauri/Cargo.toml**

In `[dependencies]` add:
```toml
llm-relay-core = { path = "../crates/llm-relay-core" }
```

- [ ] **Step 4: Replace local `mod error;` in src-tauri/src/lib.rs**

Edit `src-tauri/src/lib.rs`:
```rust
// Remove: mod error;
// Replace: pub use error::AppError;
pub use llm_relay_core::AppError;
```

- [ ] **Step 5: Update intra-crate imports in src-tauri/src/**

Run grep to find them:
```bash
grep -rn "use crate::error" src-tauri/src/
grep -rn "crate::error::" src-tauri/src/
grep -rn "crate::AppError" src-tauri/src/
```

Replace each `crate::error::AppError` (and any `crate::error::*`) with `llm_relay_core::error::AppError` (or `llm_relay_core::AppError`). Replace `use crate::error::AppError` with `use llm_relay_core::AppError`.

- [ ] **Step 6: Verify it builds**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: move error module into llm-relay-core"
```

### Task 3: Move `database.rs` into core

**Files:**
- Move: `src-tauri/src/database.rs` → `crates/llm-relay-core/src/database.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`
- Modify: all src-tauri files importing `crate::database` or `crate::Database`

- [ ] **Step 1: Move file**

```bash
git mv src-tauri/src/database.rs crates/llm-relay-core/src/database.rs
```

- [ ] **Step 2: Re-export**

Edit `crates/llm-relay-core/src/lib.rs`, append:
```rust
pub mod database;
pub use database::Database;
```

- [ ] **Step 3: Replace imports in src-tauri**

Find usages:
```bash
grep -rn "crate::database\|crate::Database\|use crate::database" src-tauri/src/
```

In `src-tauri/src/lib.rs` remove `mod database;` and `pub use database::Database;`. Replace with `pub use llm_relay_core::Database;`.

In other files (commands.rs, health.rs, proxy_server.rs, etc.) replace `crate::database::Foo` with `llm_relay_core::database::Foo`. Replace `crate::Database` with `llm_relay_core::Database`.

- [ ] **Step 4: Verify**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: move database module into llm-relay-core"
```

### Task 4: Move `config_writer.rs` and `keystore.rs` into core

**Files:**
- Move: `src-tauri/src/config_writer.rs` → `crates/llm-relay-core/src/config_writer.rs`
- Move: `src-tauri/src/keystore.rs` → `crates/llm-relay-core/src/keystore.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`
- Modify: src-tauri files importing them

- [ ] **Step 1: Move files**

```bash
git mv src-tauri/src/config_writer.rs crates/llm-relay-core/src/config_writer.rs
git mv src-tauri/src/keystore.rs crates/llm-relay-core/src/keystore.rs
```

- [ ] **Step 2: Re-export**

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod config_writer;
pub mod keystore;
```

- [ ] **Step 3: Update imports**

Find usages:
```bash
grep -rn "crate::config_writer\|crate::keystore" src-tauri/src/
```

Remove `mod config_writer;` and `mod keystore;` from `src-tauri/src/lib.rs`. Replace `crate::config_writer::*` with `llm_relay_core::config_writer::*` and same for keystore.

- [ ] **Step 4: Verify**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: move config_writer and keystore into llm-relay-core"
```

### Task 5: Move `gateway.rs` into core

**Files:**
- Move: `src-tauri/src/gateway.rs` → `crates/llm-relay-core/src/gateway.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`
- Modify: src-tauri files importing it

- [ ] **Step 1: Move file**

```bash
git mv src-tauri/src/gateway.rs crates/llm-relay-core/src/gateway.rs
```

- [ ] **Step 2: Re-export**

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod gateway;
```

- [ ] **Step 3: Update imports**

```bash
grep -rn "crate::gateway" src-tauri/src/
```

Remove `mod gateway;` from `src-tauri/src/lib.rs`. Replace `crate::gateway::*` with `llm_relay_core::gateway::*`.

- [ ] **Step 4: Verify**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: move gateway module into llm-relay-core"
```

### Task 6: Extract `EventSink` trait and refactor `health.rs` + `proxy_server.rs` to use it

The current `health.rs` and `proxy_server.rs` call `app_handle.emit("event", payload)` directly to push events to the GUI. We must abstract this so the same code can also push events to the IPC server (for TUI clients) without depending on Tauri.

**Files:**
- Create: `crates/llm-relay-core/src/events.rs`
- Move + modify: `src-tauri/src/health.rs` → `crates/llm-relay-core/src/health.rs`
- Move + modify: `src-tauri/src/proxy_server.rs` → `crates/llm-relay-core/src/proxy_server.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`
- Modify: `src-tauri/src/lib.rs` and `commands.rs` to provide a Tauri-backed `EventSink` impl

- [ ] **Step 1: Define `EventSink` trait in core**

Create `crates/llm-relay-core/src/events.rs`:

```rust
//! Trait for emitting domain events to whichever frontend is listening.
//! Tauri GUI implements this by calling `AppHandle::emit`. The IPC server
//! implements it by broadcasting `ServerFrame::Event` to all subscribed clients.

use serde::Serialize;
use std::sync::Arc;

/// A name + JSON payload event. Implementations decide where it goes.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, name: &str, payload: serde_json::Value);
}

pub type SharedEventSink = Arc<dyn EventSink>;

/// No-op sink for tests / headless contexts where no listener exists yet.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _name: &str, _payload: serde_json::Value) {}
}

/// Helper: serialize a typed payload then forward.
pub fn emit_typed<T: Serialize>(sink: &dyn EventSink, name: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(v) => sink.emit(name, v),
        Err(e) => log::error!("emit_typed serialize failed for {name}: {e}"),
    }
}
```

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod events;
pub use events::{EventSink, SharedEventSink, NullSink};
```

- [ ] **Step 2: Run a probe to find every `app_handle.emit` and `AppHandle` reference in health.rs & proxy_server.rs**

```bash
grep -n "emit\|AppHandle\|tauri::" src-tauri/src/health.rs src-tauri/src/proxy_server.rs
```

For each emit call, note the event name and payload — these become the strings passed to `EventSink::emit`. Note the `AppHandle` in the function signatures — these will become `SharedEventSink` parameters.

- [ ] **Step 3: Move `health.rs` to core, replacing AppHandle with SharedEventSink**

```bash
git mv src-tauri/src/health.rs crates/llm-relay-core/src/health.rs
```

In the moved file:
- Remove `use tauri::*;` imports.
- Replace all function parameters of type `&AppHandle` (or `tauri::AppHandle`) with `sink: &dyn EventSink` (or `&SharedEventSink` if cloned across spawns).
- Replace each `app_handle.emit("name", payload)` with `sink.emit("name", serde_json::to_value(&payload).unwrap_or_default())`.
- Replace `crate::AppState` references — health.rs currently takes `&AppState`. Inline what it needs (`db: Arc<Database>`, `switch_lock: Arc<Mutex<()>>`) as separate parameters, since `AppState` is a Tauri-side concept.
- Update imports: `crate::Database` → `crate::Database` (still works inside the core crate).

Concretely, change the public entry point signature:
```rust
// before
pub async fn health_check_loop(state: &AppState, app: &AppHandle) { ... }
// after
pub async fn health_check_loop(
    db: std::sync::Arc<crate::Database>,
    switch_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    sink: crate::SharedEventSink,
) { ... }
```

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod health;
```

- [ ] **Step 4: Move `proxy_server.rs` to core with the same pattern**

```bash
git mv src-tauri/src/proxy_server.rs crates/llm-relay-core/src/proxy_server.rs
```

In the moved file: same treatment. Replace `AppHandle` with `SharedEventSink`. The current entry point:
```rust
pub async fn start(db: Arc<Database>, app_handle: AppHandle) { ... }
```
becomes:
```rust
pub async fn start(db: std::sync::Arc<crate::Database>, sink: crate::SharedEventSink) { ... }
```

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod proxy_server;
```

- [ ] **Step 5: Implement `EventSink` for Tauri in src-tauri**

Create `src-tauri/src/tauri_sink.rs`:

```rust
use llm_relay_core::EventSink;
use tauri::{AppHandle, Emitter};

pub struct TauriSink {
    handle: AppHandle,
}

impl TauriSink {
    pub fn new(handle: AppHandle) -> Self {
        Self { handle }
    }
}

impl EventSink for TauriSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        if let Err(e) = self.handle.emit(name, payload) {
            log::warn!("tauri emit {name} failed: {e}");
        }
    }
}
```

In `src-tauri/src/lib.rs`:
- Add `mod tauri_sink;`
- Where `health::health_check_loop(state.inner(), &app_handle)` is called, replace with:
  ```rust
  let sink: llm_relay_core::SharedEventSink = std::sync::Arc::new(tauri_sink::TauriSink::new(app_handle.clone()));
  let db = state.db.clone();
  let switch_lock = state.switch_lock.clone();
  tauri::async_runtime::spawn(async move {
      llm_relay_core::health::health_check_loop(db, switch_lock, sink).await;
  });
  ```
- Same treatment for `proxy_server::start(...)`.

- [ ] **Step 6: Update remaining imports in src-tauri**

```bash
grep -rn "crate::health\|crate::proxy_server" src-tauri/src/
```

Replace `crate::health::*` → `llm_relay_core::health::*`, same for proxy_server. Remove the `mod health;` and `mod proxy_server;` lines from src-tauri/src/lib.rs.

- [ ] **Step 7: Verify**

Run: `cargo build --workspace`
Expected: clean build. Then `cargo run --bin llm-relay` (or `pnpm tauri dev`) briefly to confirm the GUI starts and the proxy responds — kill it after one health-check cycle.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: move health and proxy_server into core, introduce EventSink trait

The Tauri-specific app_handle.emit() calls are abstracted behind a trait
so the same modules can later push events to IPC clients."
```

### Task 7: Verify GUI smoke test

This task has no code changes — it forces a manual confirmation that Phase 1 didn't break anything before we move on.

- [ ] **Step 1: Run GUI**

```bash
cd /Users/zhangxian/projects/llm-relay
pnpm tauri dev
```

- [ ] **Step 2: Manual checks**

Verify the following work in the running GUI:
1. App window appears and lists existing gateways (or shows empty state)
2. Health checks run (status dots update within 60s)
3. Local proxy responds: `curl -i http://127.0.0.1:18080/v1/messages` returns the gateway's response (or auth error from the gateway, which is fine — we just want to know the proxy forwards)
4. Tray icon appears and clicking it shows the menu

Kill the dev process.

- [ ] **Step 3: If anything is broken, debug before continuing**

Use `superpowers:systematic-debugging`. Do not move to Phase 2 until smoke passes.

---


## Phase 2: Keystore fallback (encrypted file)

Linux servers often lack a Secret Service (no DBus). The new keystore probes the system keychain at startup; if it fails, it falls back to an AES-256-GCM encrypted file at `~/.llm-relay/secrets.enc`. The unified-cache pattern of the existing keystore is preserved.

### Task 8: Refactor keystore into a backend-trait shape

**Files:**
- Modify: `crates/llm-relay-core/src/keystore.rs`
- Modify: `crates/llm-relay-core/Cargo.toml` (add `aes-gcm`, `argon2`, `rand`, `rpassword`, `base64`)

- [ ] **Step 1: Add new deps**

In `crates/llm-relay-core/Cargo.toml` `[dependencies]`:
```toml
aes-gcm = "0.10"
argon2 = "0.5"
rand = "0.8"
rpassword = "7"
base64 = "0.22"
```

- [ ] **Step 2: Refactor keystore.rs to wrap the backend behind a trait**

Replace contents of `crates/llm-relay-core/src/keystore.rs` with:

```rust
//! Unified secrets store. Tries the OS keychain at first use; falls back
//! to an AES-256-GCM encrypted file when the keychain is unavailable
//! (e.g. Linux servers without DBus / Secret Service).

mod file_backend;
mod system_backend;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "llm-relay";
const ENTRY_KEY: &str = "secrets";

pub trait Backend: Send + Sync {
    fn load(&self) -> HashMap<String, String>;
    fn save(&self, map: &HashMap<String, String>);
}

static BACKEND: OnceLock<Box<dyn Backend>> = OnceLock::new();
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Initialize the keystore. Call once at startup before any get/set.
/// Tries the OS keychain first by performing a real read+write probe;
/// on failure, falls back to encrypted file.
pub fn init(config_dir: &std::path::Path) {
    let backend: Box<dyn Backend> = match system_backend::SystemBackend::probe() {
        Ok(b) => Box::new(b),
        Err(e) => {
            log::warn!("system keychain unavailable: {e}; using encrypted file at {}", config_dir.display());
            Box::new(file_backend::FileBackend::new(config_dir.join("secrets.enc")))
        }
    };
    let _ = BACKEND.set(backend);
}

fn backend() -> &'static dyn Backend {
    BACKEND
        .get()
        .expect("keystore::init() must be called before use")
        .as_ref()
}

fn load_all() -> HashMap<String, String> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(ref map) = *cache {
        return map.clone();
    }
    let map = backend().load();
    *cache = Some(map.clone());
    map
}

fn save_all(map: &HashMap<String, String>) {
    *CACHE.lock().unwrap() = Some(map.clone());
    backend().save(map);
}

pub fn set_secret(key: &str, value: &str) {
    let mut map = load_all();
    if map.get(key).map(String::as_str) == Some(value) {
        return;
    }
    map.insert(key.to_string(), value.to_string());
    save_all(&map);
}

pub fn get_secret(key: &str) -> Option<String> {
    load_all().get(key).cloned()
}

pub fn delete_secret(key: &str) {
    let mut map = load_all();
    if map.remove(key).is_some() {
        save_all(&map);
    }
}

pub fn gw_auth_key(gateway_id: &str) -> String { format!("gw:{gateway_id}:auth_key") }
pub fn gw_session_token(gateway_id: &str) -> String { format!("gw:{gateway_id}:session_token") }
pub fn active_key_value() -> String { "active:key_value".to_string() }

pub fn migrate_legacy_entries(gateway_ids: &[String]) {
    let mut map = load_all();
    let mut changed = false;
    for id in gateway_ids {
        for key in [gw_auth_key(id), gw_session_token(id)] {
            if map.contains_key(&key) { continue; }
            if let Ok(entry) = keyring::Entry::new(SERVICE, &key) {
                if let Ok(val) = entry.get_password() {
                    map.insert(key.clone(), val);
                    let _ = entry.delete_credential();
                    changed = true;
                }
            }
        }
    }
    let akv = active_key_value();
    if !map.contains_key(&akv) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &akv) {
            if let Ok(val) = entry.get_password() {
                map.insert(akv, val);
                let _ = entry.delete_credential();
                changed = true;
            }
        }
    }
    if changed { save_all(&map); }
}

pub(super) const KEYSTORE_SERVICE: &str = SERVICE;
pub(super) const KEYSTORE_ENTRY: &str = ENTRY_KEY;
```

- [ ] **Step 3: Implement system backend**

Create `crates/llm-relay-core/src/keystore/system_backend.rs`:

```rust
use super::{Backend, KEYSTORE_ENTRY, KEYSTORE_SERVICE};
use std::collections::HashMap;

pub struct SystemBackend;

impl SystemBackend {
    /// Probe by attempting a get/set on a sentinel entry. Any error means
    /// the OS keychain is not usable in this environment.
    pub fn probe() -> Result<Self, String> {
        let entry = keyring::Entry::new(KEYSTORE_SERVICE, "__probe__")
            .map_err(|e| format!("entry: {e}"))?;
        match entry.set_password("ok") {
            Ok(()) => {
                let _ = entry.delete_credential();
                Ok(Self)
            }
            Err(e) => Err(format!("write: {e}")),
        }
    }
}

impl Backend for SystemBackend {
    fn load(&self) -> HashMap<String, String> {
        match keyring::Entry::new(KEYSTORE_SERVICE, KEYSTORE_ENTRY) {
            Ok(entry) => match entry.get_password() {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => HashMap::new(),
            },
            Err(_) => HashMap::new(),
        }
    }

    fn save(&self, map: &HashMap<String, String>) {
        let json = serde_json::to_string(map).unwrap_or_else(|_| "{}".into());
        if let Ok(entry) = keyring::Entry::new(KEYSTORE_SERVICE, KEYSTORE_ENTRY) {
            if let Err(e) = entry.set_password(&json) {
                log::warn!("keystore save (system): {e}");
            }
        }
    }
}
```

- [ ] **Step 4: Stub file backend (real impl in Task 9)**

Create `crates/llm-relay-core/src/keystore/file_backend.rs`:

```rust
use super::Backend;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct FileBackend {
    pub(crate) path: PathBuf,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self { Self { path } }
}

impl Backend for FileBackend {
    fn load(&self) -> HashMap<String, String> {
        // TODO Task 9: real AES-GCM decryption
        log::warn!("file backend stubbed; load returning empty");
        HashMap::new()
    }

    fn save(&self, _map: &HashMap<String, String>) {
        // TODO Task 9
        log::warn!("file backend stubbed; save no-op");
    }
}
```

- [ ] **Step 5: Wire `keystore::init` into the Tauri app**

In `src-tauri/src/lib.rs`, inside `setup(|app| { ... })` immediately after `let app_config_dir = get_app_config_dir(); std::fs::create_dir_all(&app_config_dir).ok();`, add:

```rust
llm_relay_core::keystore::init(&app_config_dir);
```

- [ ] **Step 6: Verify**

Run: `cargo build --workspace` then `pnpm tauri dev`. Existing secrets should still load (system backend probe succeeds on macOS/Windows; on Linux desktop with DBus too). Kill after one health cycle.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(keystore): introduce backend trait, probe system keychain at init"
```

### Task 9: Implement encrypted file backend

**Files:**
- Modify: `crates/llm-relay-core/src/keystore/file_backend.rs`

The on-disk format:
```
magic   : "LLMRELAY1"      9 bytes
salt    : 16 bytes argon2 salt
nonce   : 12 bytes AES-GCM nonce
ct      : N bytes (ciphertext + 16-byte GCM tag)
```

Master password sources, in order:
1. `LLM_RELAY_KEY` env var
2. Cached in-process master key (set after first successful unlock)
3. `rpassword::prompt_password("LLM Relay master password: ")` — interactive

- [ ] **Step 1: Replace stub with real implementation**

Replace contents of `crates/llm-relay-core/src/keystore/file_backend.rs`:

```rust
use super::Backend;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Argon2, Algorithm, Version, Params};
use rand::RngCore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

const MAGIC: &[u8; 9] = b"LLMRELAY1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

pub struct FileBackend {
    path: PathBuf,
    master_key: Mutex<Option<[u8; 32]>>,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self {
        Self { path, master_key: Mutex::new(None) }
    }

    fn obtain_master_password(&self) -> Result<String, String> {
        if let Ok(p) = std::env::var("LLM_RELAY_KEY") {
            return Ok(p);
        }
        rpassword::prompt_password("LLM Relay master password: ")
            .map_err(|e| format!("password prompt failed: {e}"))
    }

    fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
        let params = Params::new(64 * 1024, 3, 1, Some(32))
            .map_err(|e| format!("argon2 params: {e}"))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut out)
            .map_err(|e| format!("argon2 derive: {e}"))?;
        Ok(out)
    }

    fn cipher_for_key(key: &[u8; 32]) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
    }

    fn read_file(&self) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        // Returns (salt, nonce, ciphertext)
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.len() < MAGIC.len() + SALT_LEN + NONCE_LEN || &bytes[..MAGIC.len()] != MAGIC {
            log::warn!("keystore file at {} has bad magic", self.path.display());
            return None;
        }
        let mut o = MAGIC.len();
        let salt = bytes[o..o + SALT_LEN].to_vec(); o += SALT_LEN;
        let nonce = bytes[o..o + NONCE_LEN].to_vec(); o += NONCE_LEN;
        let ct = bytes[o..].to_vec();
        Some((salt, nonce, ct))
    }

    fn write_file(&self, salt: &[u8], nonce: &[u8], ct: &[u8]) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(MAGIC.len() + salt.len() + nonce.len() + ct.len());
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(salt);
        buf.extend_from_slice(nonce);
        buf.extend_from_slice(ct);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, buf)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl Backend for FileBackend {
    fn load(&self) -> HashMap<String, String> {
        let Some((salt, nonce, ct)) = self.read_file() else {
            return HashMap::new();
        };
        let mut guard = self.master_key.lock().unwrap();
        let key = match *guard {
            Some(k) => k,
            None => {
                let pw = match self.obtain_master_password() {
                    Ok(p) => p,
                    Err(e) => { log::error!("{e}"); return HashMap::new(); }
                };
                match Self::derive_key(&pw, &salt) {
                    Ok(k) => { *guard = Some(k); k },
                    Err(e) => { log::error!("{e}"); return HashMap::new(); }
                }
            }
        };
        let cipher = Self::cipher_for_key(&key);
        match cipher.decrypt(Nonce::from_slice(&nonce), ct.as_ref()) {
            Ok(plain) => serde_json::from_slice(&plain).unwrap_or_default(),
            Err(e) => {
                log::error!("keystore decrypt failed: {e} (wrong password?)");
                *guard = None; // force re-prompt next time
                HashMap::new()
            }
        }
    }

    fn save(&self, map: &HashMap<String, String>) {
        let mut guard = self.master_key.lock().unwrap();
        let (salt, key) = match *guard {
            Some(k) => {
                // Reuse existing salt if file exists, else fresh
                let salt = self.read_file().map(|(s, _, _)| s).unwrap_or_else(|| {
                    let mut s = vec![0u8; SALT_LEN]; rand::thread_rng().fill_bytes(&mut s); s
                });
                (salt, k)
            }
            None => {
                let pw = match self.obtain_master_password() {
                    Ok(p) => p,
                    Err(e) => { log::error!("{e}"); return; }
                };
                let mut salt = vec![0u8; SALT_LEN]; rand::thread_rng().fill_bytes(&mut salt);
                match Self::derive_key(&pw, &salt) {
                    Ok(k) => { *guard = Some(k); (salt, k) }
                    Err(e) => { log::error!("{e}"); return; }
                }
            }
        };
        let mut nonce = vec![0u8; NONCE_LEN]; rand::thread_rng().fill_bytes(&mut nonce);
        let cipher = Self::cipher_for_key(&key);
        let plain = serde_json::to_vec(map).unwrap_or_else(|_| b"{}".to_vec());
        match cipher.encrypt(Nonce::from_slice(&nonce), plain.as_ref()) {
            Ok(ct) => {
                if let Err(e) = self.write_file(&salt, &nonce, &ct) {
                    log::error!("keystore write {} failed: {e}", self.path.display());
                }
            }
            Err(e) => log::error!("keystore encrypt failed: {e}"),
        }
    }
}
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(keystore): implement AES-256-GCM encrypted file backend"
```

### Task 10: Test the keystore round-trip

**Files:**
- Create: `crates/llm-relay-core/tests/keystore_file.rs`

- [ ] **Step 1: Write failing test**

```rust
//! Tests the file backend in isolation (system backend requires a real keychain).

use llm_relay_core::keystore::{file_backend_for_test, Backend};
use std::collections::HashMap;

mod helper {
    pub fn unique_tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("llm-relay-test-{}-{}", name, std::process::id()));
        p
    }
}

#[test]
fn round_trip_with_env_password() {
    std::env::set_var("LLM_RELAY_KEY", "test-pw-123");
    let path = helper::unique_tmp("rt").join("secrets.enc");
    let _ = std::fs::remove_file(&path);

    let be = file_backend_for_test(path.clone());
    let mut m = HashMap::new();
    m.insert("k1".to_string(), "v1".to_string());
    m.insert("k2".to_string(), "v2".to_string());
    be.save(&m);

    // Fresh backend reading the same file with same env password
    let be2 = file_backend_for_test(path);
    let loaded = be2.load();
    assert_eq!(loaded, m);
}

#[test]
fn wrong_password_yields_empty_map() {
    std::env::set_var("LLM_RELAY_KEY", "correct");
    let path = helper::unique_tmp("wp").join("secrets.enc");
    let _ = std::fs::remove_file(&path);

    let be = file_backend_for_test(path.clone());
    let mut m = HashMap::new();
    m.insert("k".to_string(), "v".to_string());
    be.save(&m);

    std::env::set_var("LLM_RELAY_KEY", "wrong");
    let be2 = file_backend_for_test(path);
    let loaded = be2.load();
    assert!(loaded.is_empty(), "wrong password should fail to decrypt");
}
```

- [ ] **Step 2: Expose the test helper**

Append to `crates/llm-relay-core/src/keystore.rs`:

```rust
/// Test-only constructor exposing the file backend without going through `init()`.
#[doc(hidden)]
pub fn file_backend_for_test(path: std::path::PathBuf) -> impl Backend {
    file_backend::FileBackend::new(path)
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p llm-relay-core --test keystore_file`
Expected: both tests pass. Note: `wrong_password_yields_empty_map` is order-sensitive due to env vars — that's fine for now (single-threaded test runner).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(keystore): round-trip encrypted file backend"
```

---


## Phase 3: IPC protocol & codec

### Task 11: Define IPC protocol types

**Files:**
- Create: `crates/llm-relay-core/src/ipc/mod.rs`
- Create: `crates/llm-relay-core/src/ipc/protocol.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`

- [ ] **Step 1: Create module skeleton**

Create `crates/llm-relay-core/src/ipc/mod.rs`:
```rust
pub mod codec;
pub mod protocol;

pub use protocol::*;
```

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod ipc;
```

- [ ] **Step 2: Define protocol types**

Create `crates/llm-relay-core/src/ipc/protocol.rs`:

```rust
//! IPC protocol between agent (server) and TUI/clients.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientFrame {
    pub request_id: u64,
    pub payload: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    Response { request_id: u64, payload: Response },
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    GetSnapshot,
    Subscribe { topics: Vec<Topic> },
    Unsubscribe { topics: Vec<Topic> },
    AddGateway(GatewayInput),
    UpdateGateway { id: Uuid, fields: GatewayUpdate },
    DeleteGateway { id: Uuid },
    SetActive { gateway_id: Uuid, key_id: Uuid, models: ModelSelection },
    ClearActive,
    SetAutoFailover(bool),
    Reorder(Vec<Uuid>),
    GetUsage { range: TimeRange, gateway_id: Option<Uuid> },
    GetTrafficLog { gateway_id: Option<Uuid> },
    StartLogin { gateway_id: Uuid },
    CancelLogin { gateway_id: Uuid },
    FetchKeys { gateway_id: Uuid },
    FetchModels { gateway_id: Uuid, key_id: Uuid },
    GetSettings,
    UpdateSettings(SettingsUpdate),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Snapshot(Snapshot),
    Ok,
    Error { message: String },
    LoginInitiated {
        device_code: String,
        user_code: String,
        verification_url: String,
        expires_at: DateTime<Utc>,
        interval_secs: u64,
    },
    Keys(Vec<KeyInfo>),
    Models(ModelCatalog),
    Settings(Settings),
    Usage(UsageReport),
    TrafficLog(Vec<TrafficEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    HealthChanged { gateway_id: Uuid, status: HealthStatus },
    ActiveChanged { gateway_id: Option<Uuid> },
    TrafficError(TrafficEntry),
    UsageDelta { gateway_id: Uuid, model: String, input: u64, output: u64, cache: u64 },
    LoginCompleted { gateway_id: Uuid, user_name: Option<String> },
    LoginFailed { gateway_id: Uuid, reason: String },
    LoginExpired { gateway_id: Uuid },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Topic { Health, Active, Traffic, Usage, Login }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayInput {
    pub name: String,
    pub url: String,
    pub auth_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayUpdate {
    pub name: Option<String>,
    pub url: Option<String>,
    pub auth_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub gateways: Vec<GatewayView>,
    pub active: Option<ActiveView>,
    pub auto_failover: bool,
    pub agent_pid: u32,
    pub agent_started_at: DateTime<Utc>,
    pub proxy_port: u16,
    pub keystore_kind: KeystoreKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayView {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub sort_order: i32,
    pub health: Option<HealthStatus>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub model_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveView {
    pub gateway_id: Uuid,
    pub key_id: Uuid,
    pub key_name: String,
    pub models: ModelSelection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelSelection {
    pub claude: Option<String>,
    pub claude_small: Option<String>,
    pub codex: Option<String>,
    pub gemini: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub claude: Vec<String>,
    pub codex: Vec<String>,
    pub gemini: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus { Healthy, Degraded, Down, Unknown }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeystoreKind { System, EncryptedFile }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange { Today, Week, Days7, Days30 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub range: TimeRange,
    pub rows: Vec<UsageRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRow {
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficEntry {
    pub at: DateTime<Utc>,
    pub gateway_id: Uuid,
    pub gateway_name: String,
    pub status: u16,
    pub path: String,
    pub latency_ms: u32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub client_name: String,
    pub auto_failover: bool,
    pub launch_at_login: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsUpdate {
    pub client_name: Option<String>,
    pub launch_at_login: Option<bool>,
}
```

- [ ] **Step 3: Verify**

Run: `cargo build --workspace`
Expected: clean build (some warnings about unused variants are fine).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(ipc): define protocol types (ClientFrame/ServerFrame/Request/Response/Event)"
```

### Task 12: Implement length-prefixed JSON codec

**Files:**
- Create: `crates/llm-relay-core/src/ipc/codec.rs`
- Modify: `crates/llm-relay-core/Cargo.toml` (add `tokio-util` for length-delimited if desired — we'll write our own minimal one)

- [ ] **Step 1: Implement codec**

Create `crates/llm-relay-core/src/ipc/codec.rs`:

```rust
//! Frame format: 4-byte big-endian length prefix, then UTF-8 JSON body.

use serde::{de::DeserializeOwned, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap to avoid runaway allocation from a corrupt/malicious peer.
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if body.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

pub async fn read_frame<R, T>(r: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame length"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

- [ ] **Step 2: Verify**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(ipc): length-prefixed JSON codec"
```

### Task 13: Round-trip codec test

**Files:**
- Create: `crates/llm-relay-core/tests/ipc_roundtrip.rs`

- [ ] **Step 1: Write test**

```rust
use chrono::Utc;
use llm_relay_core::ipc::codec::{read_frame, write_frame};
use llm_relay_core::ipc::protocol::*;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[tokio::test]
async fn round_trip_request_and_event() {
    let (mut a, mut b) = duplex(8192);

    let req = ClientFrame {
        request_id: 42,
        payload: Request::SetActive {
            gateway_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            models: ModelSelection { claude: Some("sonnet".into()), ..Default::default() },
        },
    };
    write_frame(&mut a, &req).await.unwrap();
    a.shutdown().await.ok();

    let got: ClientFrame = read_frame(&mut b).await.unwrap();
    assert_eq!(got.request_id, 42);
    match got.payload {
        Request::SetActive { models, .. } => assert_eq!(models.claude.as_deref(), Some("sonnet")),
        _ => panic!("wrong variant"),
    }
}

#[tokio::test]
async fn server_frame_event_kind_intact() {
    let (mut a, mut b) = duplex(4096);
    let frame = ServerFrame::Event(Event::HealthChanged {
        gateway_id: Uuid::new_v4(),
        status: HealthStatus::Healthy,
    });
    write_frame(&mut a, &frame).await.unwrap();
    a.shutdown().await.ok();

    let got: ServerFrame = read_frame(&mut b).await.unwrap();
    match got {
        ServerFrame::Event(Event::HealthChanged { status, .. }) => {
            assert_eq!(status, HealthStatus::Healthy);
        }
        _ => panic!("wrong frame"),
    }
}

#[tokio::test]
async fn server_frame_response_carries_request_id() {
    let (mut a, mut b) = duplex(4096);
    let frame = ServerFrame::Response {
        request_id: 7,
        payload: Response::LoginInitiated {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_url: "https://gw/device/login".into(),
            expires_at: Utc::now(),
            interval_secs: 5,
        },
    };
    write_frame(&mut a, &frame).await.unwrap();
    a.shutdown().await.ok();

    let got: ServerFrame = read_frame(&mut b).await.unwrap();
    match got {
        ServerFrame::Response { request_id, payload: Response::LoginInitiated { user_code, .. } } => {
            assert_eq!(request_id, 7);
            assert_eq!(user_code, "ABCD-1234");
        }
        _ => panic!("wrong frame"),
    }
}

#[tokio::test]
async fn rejects_oversize_frame() {
    let (mut a, mut b) = duplex(64);
    // Fake frame: claim 100 MB length
    a.write_all(&100_000_000u32.to_be_bytes()).await.unwrap();
    a.shutdown().await.ok();

    let res: std::io::Result<ClientFrame> = read_frame(&mut b).await;
    assert!(res.is_err());
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p llm-relay-core --test ipc_roundtrip`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(ipc): codec round-trip + oversize rejection"
```

### Task 14: Transport layer (interprocess local socket)

**Files:**
- Create: `crates/llm-relay-core/src/ipc/transport.rs`
- Modify: `crates/llm-relay-core/src/ipc/mod.rs`
- Modify: `crates/llm-relay-core/Cargo.toml` (add `interprocess`)

- [ ] **Step 1: Add deps**

In `crates/llm-relay-core/Cargo.toml`:
```toml
interprocess = { version = "2", features = ["tokio"] }
```

- [ ] **Step 2: Implement transport**

Create `crates/llm-relay-core/src/ipc/transport.rs`:

```rust
//! Cross-platform local socket transport.
//! Unix: socket file at the given path.
//! Windows: named pipe whose name is derived from the path's file_name.

use interprocess::local_socket::{
    tokio::{prelude::*, Stream, Listener},
    GenericFilePath, GenericNamespaced, ListenerOptions, ToFsName, ToNsName,
};
use std::io;
use std::path::Path;

/// Build a listener bound to the given socket path (Unix) or namespaced name (Windows).
pub fn build_listener(path: &Path) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        // If a stale socket file exists, remove it first.
        if path.exists() { let _ = std::fs::remove_file(path); }
        let name = path.to_fs_name::<GenericFilePath>()?;
        ListenerOptions::new().name(name).create_tokio()
    }
    #[cfg(windows)]
    {
        let pipe_name = format!(
            r"llm-relay-agent-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("default")
        );
        let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
        ListenerOptions::new().name(name).create_tokio()
    }
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    {
        let name = path.to_fs_name::<GenericFilePath>()?;
        Stream::connect(name).await
    }
    #[cfg(windows)]
    {
        let pipe_name = format!(
            r"llm-relay-agent-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("default")
        );
        let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
        Stream::connect(name).await
    }
}
```

In `crates/llm-relay-core/src/ipc/mod.rs` add:
```rust
pub mod transport;
```

- [ ] **Step 3: Verify**

Run: `cargo build --workspace`
Expected: clean build. (If interprocess API names drift, consult its docs; the goal is "build a listener at a path / connect to a path".)

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(ipc): cross-platform local socket transport via interprocess"
```

---


## Phase 4: Service façade & EventSink wiring

The `Service` struct is the high-level entry point both the GUI and the agent call. It owns the database handle, the switch lock, and the event sink, and exposes async methods named to match the IPC `Request` variants. This keeps the IPC server thin (decode → dispatch → encode).

### Task 15: Implement `Service` façade in core

**Files:**
- Create: `crates/llm-relay-core/src/service.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`

- [ ] **Step 1: Create Service**

Create `crates/llm-relay-core/src/service.rs`:

```rust
//! High-level façade used by both the Tauri GUI and the IPC agent.
//! Each public method maps 1:1 to an `ipc::Request` variant.

use crate::ipc::protocol::*;
use crate::{AppError, Database, SharedEventSink};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct Service {
    pub db: Arc<Database>,
    pub switch_lock: Arc<Mutex<()>>,
    pub sink: SharedEventSink,
}

impl Service {
    pub fn new(db: Arc<Database>, sink: SharedEventSink) -> Self {
        Self { db, sink, switch_lock: Arc::new(Mutex::new(())) }
    }

    pub async fn snapshot(&self, agent_pid: u32, agent_started_at: chrono::DateTime<chrono::Utc>, proxy_port: u16, keystore_kind: KeystoreKind) -> Result<Snapshot, AppError> {
        let gateways = self.list_gateway_views()?;
        let active = self.active_view()?;
        let auto_failover = self.db.get_auto_failover().unwrap_or(false);
        Ok(Snapshot { gateways, active, auto_failover, agent_pid, agent_started_at, proxy_port, keystore_kind })
    }

    fn list_gateway_views(&self) -> Result<Vec<GatewayView>, AppError> {
        // Adapt to the existing Database API. The current `list_gateways` returns
        // a richer struct; we project it to GatewayView here.
        let raw = self.db.list_gateways()?; // existing method
        let cache = self.db.read_health_cache_all().unwrap_or_default();
        Ok(raw.into_iter().map(|g| {
            let h = cache.iter().find(|h| h.gateway_id == g.id);
            GatewayView {
                id: g.id,
                name: g.name,
                url: g.url,
                sort_order: g.sort_order,
                health: h.map(|h| map_health_status(&h.status)),
                last_check_at: h.and_then(|h| h.checked_at),
                model_count: h.and_then(|h| h.model_count),
            }
        }).collect())
    }

    fn active_view(&self) -> Result<Option<ActiveView>, AppError> {
        let Some(active) = self.db.get_active_config()? else { return Ok(None); };
        let key_name = self.db.get_key_name(active.gateway_id, active.key_id).unwrap_or_default();
        Ok(Some(ActiveView {
            gateway_id: active.gateway_id,
            key_id: active.key_id,
            key_name,
            models: ModelSelection {
                claude: active.claude_model,
                claude_small: active.claude_small_model,
                codex: active.codex_model,
                gemini: active.gemini_model,
            },
        }))
    }

    pub async fn add_gateway(&self, input: GatewayInput) -> Result<Uuid, AppError> {
        // Reuse existing DB call signature; rename arguments as needed.
        let id = self.db.add_gateway(&input.name, &input.url, &input.auth_key)?;
        crate::keystore::set_secret(&crate::keystore::gw_auth_key(&id.to_string()), &input.auth_key);
        Ok(id)
    }

    pub async fn update_gateway(&self, id: Uuid, fields: GatewayUpdate) -> Result<(), AppError> {
        self.db.update_gateway(id, fields.name.as_deref(), fields.url.as_deref())?;
        if let Some(ak) = fields.auth_key.as_deref() {
            crate::keystore::set_secret(&crate::keystore::gw_auth_key(&id.to_string()), ak);
        }
        Ok(())
    }

    pub async fn delete_gateway(&self, id: Uuid) -> Result<(), AppError> {
        self.db.delete_gateway(id)?;
        crate::keystore::delete_secret(&crate::keystore::gw_auth_key(&id.to_string()));
        crate::keystore::delete_secret(&crate::keystore::gw_session_token(&id.to_string()));
        Ok(())
    }

    pub async fn set_active(&self, gateway_id: Uuid, key_id: Uuid, models: ModelSelection) -> Result<(), AppError> {
        let _g = self.switch_lock.lock().await;
        self.db.set_active_config(gateway_id, key_id, &models.claude, &models.claude_small, &models.codex, &models.gemini)?;
        // Reuse existing config_writer to update CLI configs.
        crate::config_writer::apply_active(&self.db)?;
        crate::events::emit_typed(&*self.sink, "active_changed", &Event::ActiveChanged { gateway_id: Some(gateway_id) });
        Ok(())
    }

    pub async fn clear_active(&self) -> Result<(), AppError> {
        let _g = self.switch_lock.lock().await;
        self.db.clear_active_config()?;
        crate::config_writer::clear_active(&self.db)?;
        crate::events::emit_typed(&*self.sink, "active_changed", &Event::ActiveChanged { gateway_id: None });
        Ok(())
    }

    pub async fn set_auto_failover(&self, on: bool) -> Result<(), AppError> {
        self.db.set_auto_failover(on)?;
        Ok(())
    }

    pub async fn reorder(&self, ids: Vec<Uuid>) -> Result<(), AppError> {
        self.db.reorder_gateways(&ids)?;
        Ok(())
    }

    pub async fn fetch_keys(&self, gateway_id: Uuid) -> Result<Vec<KeyInfo>, AppError> {
        let g = self.db.get_gateway(gateway_id)?.ok_or_else(|| AppError::NotFound("gateway".into()))?;
        let session = crate::keystore::get_secret(&crate::keystore::gw_session_token(&gateway_id.to_string()));
        let keys = crate::gateway::fetch_keys(&g.url, session.as_deref()).await?;
        Ok(keys.into_iter().map(|k| KeyInfo { id: k.id, name: k.name }).collect())
    }

    pub async fn fetch_models(&self, gateway_id: Uuid, key_id: Uuid) -> Result<ModelCatalog, AppError> {
        let g = self.db.get_gateway(gateway_id)?.ok_or_else(|| AppError::NotFound("gateway".into()))?;
        let key_value = self.db.get_key_value(gateway_id, key_id)?
            .ok_or_else(|| AppError::NotFound("key".into()))?;
        let cat = crate::gateway::fetch_models(&g.url, &key_value).await?;
        Ok(ModelCatalog {
            claude: cat.claude.unwrap_or_default(),
            codex: cat.codex.unwrap_or_default(),
            gemini: cat.gemini.unwrap_or_default(),
        })
    }

    pub async fn get_usage(&self, range: TimeRange, gateway_id: Option<Uuid>) -> Result<UsageReport, AppError> {
        let rows = self.db.get_usage_stats(range_to_hours(range), gateway_id)?;
        Ok(UsageReport {
            range,
            rows: rows.into_iter().map(|r| UsageRow {
                model: r.model, input: r.input, output: r.output, cache: r.cache, total: r.input + r.output,
            }).collect(),
        })
    }

    pub async fn get_traffic_log(&self, gateway_id: Option<Uuid>) -> Result<Vec<TrafficEntry>, AppError> {
        let logs = self.db.get_traffic_logs(gateway_id)?;
        Ok(logs.into_iter().map(|l| TrafficEntry {
            at: l.at, gateway_id: l.gateway_id, gateway_name: l.gateway_name,
            status: l.status, path: l.path, latency_ms: l.latency_ms, detail: l.detail,
        }).collect())
    }

    pub async fn get_settings(&self) -> Result<Settings, AppError> {
        Ok(Settings {
            client_name: self.db.get_setting("client_name").unwrap_or_default().unwrap_or_default(),
            auto_failover: self.db.get_auto_failover().unwrap_or(false),
            launch_at_login: self.db.get_setting("launch_at_login")?.as_deref() == Some("true"),
        })
    }

    pub async fn update_settings(&self, u: SettingsUpdate) -> Result<(), AppError> {
        if let Some(n) = u.client_name { self.db.set_setting("client_name", &n)?; }
        if let Some(b) = u.launch_at_login { self.db.set_setting("launch_at_login", if b {"true"} else {"false"})?; }
        Ok(())
    }
}

fn map_health_status(s: &str) -> HealthStatus {
    match s {
        "healthy" => HealthStatus::Healthy,
        "degraded" => HealthStatus::Degraded,
        "down" => HealthStatus::Down,
        _ => HealthStatus::Unknown,
    }
}

fn range_to_hours(r: TimeRange) -> i64 {
    match r { TimeRange::Today => 24, TimeRange::Week => 24*7, TimeRange::Days7 => 24*7, TimeRange::Days30 => 24*30 }
}
```

> **Note for the implementer:** Several method names referenced above (`db.list_gateways`, `db.get_active_config`, `db.set_active_config`, `db.read_health_cache_all`, `db.get_key_name`, `db.get_key_value`, `db.get_traffic_logs`, `db.get_usage_stats`, `db.set_setting`, `db.get_setting`, `db.set_auto_failover`, `db.get_auto_failover`, `db.reorder_gateways`, `db.update_gateway`, `db.delete_gateway`, `db.add_gateway`, `db.get_gateway`, `db.clear_active_config`) reflect the existing Database API surface used by `commands.rs`. Inspect `crates/llm-relay-core/src/database.rs` to confirm exact signatures; if a method has a slightly different name (e.g. `list_gateways` vs `get_gateways`), adapt the wrapper here — do NOT rename the underlying DB function. Same for `gateway::fetch_keys`, `gateway::fetch_models`, and `config_writer::apply_active` / `clear_active`. Add small adapter functions if shapes differ rather than churning callers.

- [ ] **Step 2: Export from lib**

Append to `crates/llm-relay-core/src/lib.rs`:
```rust
pub mod service;
pub use service::Service;
```

- [ ] **Step 3: Verify**

Run: `cargo build --workspace`

If compile errors mention DB method signatures: inspect `crates/llm-relay-core/src/database.rs` and adapt the wrapper to the actual signatures. If a piece of behavior the wrapper assumes (e.g. a `get_key_name` lookup) does not yet exist as a separate method, add a small helper to `database.rs` rather than embedding raw SQL in `service.rs`.

Expected after fixes: clean build.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(core): introduce Service façade with one method per IPC Request variant"
```

### Task 16: Migrate Tauri commands to use Service

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

This task replaces the current pattern (Tauri commands talk to `Database` + `keystore` + `gateway` + `config_writer` directly) with: Tauri commands hold an `Arc<Service>` and just forward.

- [ ] **Step 1: Add `service: Arc<Service>` to AppState**

Edit `src-tauri/src/lib.rs`:

```rust
pub struct AppState {
    pub db: Arc<Database>,
    pub switch_lock: Arc<tokio::sync::Mutex<()>>,
    pub service: Arc<llm_relay_core::Service>,
}
```

In `setup`, build the service after constructing `db` and `sink`:
```rust
let service = std::sync::Arc::new(llm_relay_core::Service::new(db.clone(), sink.clone()));
let state = AppState { db: db.clone(), switch_lock: Arc::new(tokio::sync::Mutex::new(())), service };
```

(The `switch_lock` field is now redundant with `service.switch_lock`; keep both for the moment to avoid touching legacy callers, then remove in a later cleanup task.)

- [ ] **Step 2: Migrate one command at a time**

Open `src-tauri/src/commands.rs`. For each command, replace the body that touches `Database` / `keystore` / `gateway` directly with a call to `state.service.foo(...).await`. Example for `add_gateway`:

```rust
#[tauri::command]
pub async fn add_gateway(
    state: tauri::State<'_, crate::AppState>,
    name: String, url: String, auth_key: String,
) -> Result<Uuid, String> {
    state.service
        .add_gateway(llm_relay_core::ipc::protocol::GatewayInput { name, url, auth_key })
        .await
        .map_err(|e| e.to_string())
}
```

Repeat for `list_gateways`, `update_gateway`, `delete_gateway`, `reorder_gateways`, `fetch_keys`, `fetch_models`, `apply_config`, `clear_config`, `get_active_config_cmd`, `get_settings`, `update_settings`, `get_traffic_logs`, `get_usage_stats`. Leave `start_device_login` / `poll_device_login` for Task 23 (they get a different lifecycle on the agent side, but the GUI continues to call the existing inline implementation).

- [ ] **Step 3: Verify GUI still works**

Run: `cargo build --workspace`, then `pnpm tauri dev`. Add/remove a gateway from the UI; confirm no regressions.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(gui): route Tauri commands through Service façade"
```

### Task 17: Make `health` and `proxy_server` accept `Service` (event emission unchanged)

**Files:**
- Modify: `crates/llm-relay-core/src/health.rs`
- Modify: `crates/llm-relay-core/src/proxy_server.rs`
- Modify: `src-tauri/src/lib.rs`

Phase 1's refactor changed these to accept `(db, switch_lock, sink)`. We now consolidate to `(service)` so the agent and GUI both pass a `Service` clone. The sink remains reachable as `service.sink`.

- [ ] **Step 1: Change signatures**

`health.rs`:
```rust
pub async fn health_check_loop(service: crate::Service) {
    // body uses service.db, service.switch_lock, service.sink
}
```

`proxy_server.rs`:
```rust
pub async fn start(service: crate::Service) {
    // body uses service.db, service.sink
}
```

Update internal references inside both files (replace any `db` → `service.db.clone()`, `sink` → `&*service.sink`, etc).

- [ ] **Step 2: Update Tauri call sites**

In `src-tauri/src/lib.rs`:
```rust
let svc_for_proxy = state.service.clone();
tauri::async_runtime::spawn(async move { llm_relay_core::proxy_server::start(svc_for_proxy).await; });

let svc_for_health = state.service.clone();
tauri::async_runtime::spawn(async move { llm_relay_core::health::health_check_loop(svc_for_health).await; });
```

- [ ] **Step 3: Verify**

`cargo build --workspace` then `pnpm tauri dev`; confirm one health cycle still emits the `health_changed` event observed in the UI.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(core): health and proxy_server take Service instead of loose deps"
```

---


## Phase 5: Agent binary (lifecycle + IPC server)

The agent is a long-lived process that owns the proxy + health loop and serves IPC clients. It guarantees mutual exclusion via a cross-platform file lock plus the proxy port bind.

### Task 18: Define paths and lifecycle helpers

**Files:**
- Create: `crates/llm-relay-core/src/paths.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`

- [ ] **Step 1: Create paths module**

Create `crates/llm-relay-core/src/paths.rs`:

```rust
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".llm-relay")
}

pub fn pid_file() -> PathBuf { config_dir().join("agent.pid") }
pub fn lock_file() -> PathBuf { config_dir().join("agent.lock") }
pub fn sock_file() -> PathBuf { config_dir().join("agent.sock") }
pub fn log_file() -> PathBuf { config_dir().join("agent.log") }
pub fn db_file() -> PathBuf { config_dir().join("config.db") }

pub const PROXY_PORT: u16 = 18080;
```

Append to `lib.rs`:
```rust
pub mod paths;
```

- [ ] **Step 2: Verify**

`cargo build --workspace`. Commit:

```bash
git add -A
git commit -m "feat(core): centralize ~/.llm-relay path constants"
```

### Task 19: Lifecycle (lock, pid, port-probe)

**Files:**
- Create: `crates/llm-relay-agent/src/lifecycle.rs`
- Modify: `crates/llm-relay-agent/Cargo.toml` (add `fs2`)

- [ ] **Step 1: Add fs2 dep**

In `crates/llm-relay-agent/Cargo.toml`:
```toml
fs2 = "0.4"
llm-relay-core = { path = "../llm-relay-core" }
chrono = { workspace = true }
```

- [ ] **Step 2: Implement lifecycle**

Create `crates/llm-relay-agent/src/lifecycle.rs`:

```rust
//! Agent lifecycle: file lock + pidfile + port probe + cleanup.

use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use llm_relay_core::paths;
use std::fs::{File, OpenOptions};
use std::io::Write;

pub struct LifecycleGuard {
    _lock: File,
}

impl LifecycleGuard {
    /// Acquire the agent lock + verify port + write pidfile.
    /// Returns a guard whose Drop releases the lock and removes pid/sock files.
    pub fn acquire() -> Result<Self> {
        std::fs::create_dir_all(paths::config_dir())?;

        // 1. Try the cross-platform exclusive lock.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(paths::lock_file())
            .with_context(|| format!("open {}", paths::lock_file().display()))?;
        lock.try_lock_exclusive()
            .map_err(|_| anyhow!("another llm-relay-agent already holds {}", paths::lock_file().display()))?;

        // 2. Probe port 18080. If bound by something else, abort and inform user.
        match std::net::TcpListener::bind(("127.0.0.1", paths::PROXY_PORT)) {
            Ok(l) => drop(l),
            Err(e) => return Err(anyhow!(
                "port {} in use ({}). Is the GUI running? Stop it first.",
                paths::PROXY_PORT, e
            )),
        }

        // 3. Remove stale socket file from a previous unclean exit.
        let _ = std::fs::remove_file(paths::sock_file());

        // 4. Write pidfile.
        let pid = std::process::id();
        let mut pf = File::create(paths::pid_file())?;
        writeln!(pf, "{pid}")?;

        Ok(Self { _lock: lock })
    }

    pub fn pid(&self) -> u32 { std::process::id() }
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(paths::pid_file());
        let _ = std::fs::remove_file(paths::sock_file());
        // Lock is released when File drops.
    }
}

/// Read pidfile and check if the named process is alive.
/// Returns Some(pid) only if the process exists.
pub fn live_agent_pid() -> Option<u32> {
    let s = std::fs::read_to_string(paths::pid_file()).ok()?;
    let pid: u32 = s.trim().parse().ok()?;
    if process_alive(pid) { Some(pid) } else { None }
}

#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    // signal 0 = check existence
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { false } else { CloseHandle(h); true }
    }
}
```

- [ ] **Step 3: Add platform deps**

In `crates/llm-relay-agent/Cargo.toml`:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_Foundation", "Win32_System_Threading"] }
```

- [ ] **Step 4: Verify**

`cargo build --workspace`. Commit:

```bash
git add -A
git commit -m "feat(agent): lifecycle guard (file lock, port probe, pidfile, stale-sock cleanup)"
```

### Task 20: IPC server skeleton (accept loop + per-connection handler)

**Files:**
- Create: `crates/llm-relay-agent/src/ipc_server.rs`

- [ ] **Step 1: Write the server**

Create `crates/llm-relay-agent/src/ipc_server.rs`:

```rust
use anyhow::Result;
use llm_relay_core::ipc::codec::{read_frame, write_frame};
use llm_relay_core::ipc::protocol::*;
use llm_relay_core::ipc::transport;
use llm_relay_core::Service;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, Mutex};

/// A bus that the agent's domain code pushes events into; each connected
/// client subscribes to it (filtered by their Topic set).
#[derive(Clone)]
pub struct EventBus(pub broadcast::Sender<Event>);

impl EventBus {
    pub fn new() -> Self { Self(broadcast::channel(1024).0) }
    pub fn publish(&self, ev: Event) { let _ = self.0.send(ev); }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> { self.0.subscribe() }
}

/// Implement EventSink by forwarding emit() into the bus. We pattern-match
/// the JSON to convert the loose (name, json) into a typed Event.
pub struct BusSink { pub bus: EventBus }
impl llm_relay_core::EventSink for BusSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        let ev: Option<Event> = match name {
            "health_changed" => serde_json::from_value(payload).ok().map(|v: HealthChanged| Event::HealthChanged { gateway_id: v.gateway_id, status: v.status }),
            "active_changed" => serde_json::from_value(payload).ok().map(|v: ActiveChanged| Event::ActiveChanged { gateway_id: v.gateway_id }),
            "traffic_error" => serde_json::from_value::<TrafficEntry>(payload).ok().map(Event::TrafficError),
            "usage_delta" => serde_json::from_value::<UsageDelta>(payload).ok().map(|v| Event::UsageDelta { gateway_id: v.gateway_id, model: v.model, input: v.input, output: v.output, cache: v.cache }),
            "login_completed" => serde_json::from_value::<LoginCompleted>(payload).ok().map(|v| Event::LoginCompleted { gateway_id: v.gateway_id, user_name: v.user_name }),
            "login_failed" => serde_json::from_value::<LoginFailed>(payload).ok().map(|v| Event::LoginFailed { gateway_id: v.gateway_id, reason: v.reason }),
            "login_expired" => serde_json::from_value::<LoginExpired>(payload).ok().map(|v| Event::LoginExpired { gateway_id: v.gateway_id }),
            other => { log::warn!("unmapped event name: {other}"); None }
        };
        if let Some(ev) = ev { self.bus.publish(ev); }
    }
}

#[derive(serde::Deserialize)] struct HealthChanged { gateway_id: uuid::Uuid, status: HealthStatus }
#[derive(serde::Deserialize)] struct ActiveChanged { gateway_id: Option<uuid::Uuid> }
#[derive(serde::Deserialize)] struct UsageDelta { gateway_id: uuid::Uuid, model: String, input: u64, output: u64, cache: u64 }
#[derive(serde::Deserialize)] struct LoginCompleted { gateway_id: uuid::Uuid, user_name: Option<String> }
#[derive(serde::Deserialize)] struct LoginFailed { gateway_id: uuid::Uuid, reason: String }
#[derive(serde::Deserialize)] struct LoginExpired { gateway_id: uuid::Uuid }

pub struct ServerCtx {
    pub service: Service,
    pub bus: EventBus,
    pub agent_started_at: chrono::DateTime<chrono::Utc>,
    pub agent_pid: u32,
    pub keystore_kind: KeystoreKind,
    pub shutdown: Arc<tokio::sync::Notify>,
}

pub async fn run(sock_path: &Path, ctx: ServerCtx) -> Result<()> {
    let listener = transport::build_listener(sock_path)?;
    log::info!("ipc listening at {}", sock_path.display());

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => {
                log::info!("ipc server shutting down");
                return Ok(());
            }
            res = listener.accept() => {
                let stream = match res {
                    Ok(s) => s,
                    Err(e) => { log::warn!("accept: {e}"); continue; }
                };
                let ctx = ctx.clone_for_conn();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, ctx).await {
                        log::warn!("conn ended: {e}");
                    }
                });
            }
        }
    }
}

impl ServerCtx {
    fn clone_for_conn(&self) -> ConnCtx {
        ConnCtx {
            service: self.service.clone(),
            bus: self.bus.clone(),
            agent_started_at: self.agent_started_at,
            agent_pid: self.agent_pid,
            keystore_kind: self.keystore_kind,
        }
    }
}

#[derive(Clone)]
struct ConnCtx {
    service: Service,
    bus: EventBus,
    agent_started_at: chrono::DateTime<chrono::Utc>,
    agent_pid: u32,
    keystore_kind: KeystoreKind,
}

async fn handle_conn<S>(mut stream: S, ctx: ConnCtx) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {
    use tokio::io::split;
    let (mut rd, mut wr) = split(stream);
    let topics: Arc<Mutex<HashSet<Topic>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut events_rx = ctx.bus.subscribe();
    let topics_for_pump = topics.clone();
    let (write_tx, mut write_rx) = tokio::sync::mpsc::channel::<ServerFrame>(64);

    // Writer task: serializes everything onto the wire.
    let writer = tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            if let Err(e) = write_frame(&mut wr, &frame).await { log::warn!("write: {e}"); break; }
        }
    });

    // Event pump: relays bus events to the writer if subscribed.
    let pump_tx = write_tx.clone();
    let pump = tokio::spawn(async move {
        loop {
            match events_rx.recv().await {
                Ok(ev) => {
                    let topic = match &ev {
                        Event::HealthChanged { .. } => Topic::Health,
                        Event::ActiveChanged { .. } => Topic::Active,
                        Event::TrafficError { .. } => Topic::Traffic,
                        Event::UsageDelta { .. } => Topic::Usage,
                        Event::LoginCompleted { .. } | Event::LoginFailed { .. } | Event::LoginExpired { .. } => Topic::Login,
                    };
                    if topics_for_pump.lock().await.contains(&topic) {
                        let _ = pump_tx.send(ServerFrame::Event(ev)).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => log::warn!("event lag {n}"),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Request loop.
    loop {
        let frame: ClientFrame = match read_frame(&mut rd).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        let resp = dispatch(&ctx, frame.payload, &topics).await;
        if write_tx.send(ServerFrame::Response { request_id: frame.request_id, payload: resp }).await.is_err() {
            break;
        }
    }

    drop(write_tx);
    let _ = writer.await;
    pump.abort();
    Ok(())
}

async fn dispatch(ctx: &ConnCtx, req: Request, topics: &Arc<Mutex<HashSet<Topic>>>) -> Response {
    macro_rules! ok_or_err { ($e:expr) => { match $e { Ok(_) => Response::Ok, Err(e) => Response::Error { message: e.to_string() } } } }

    match req {
        Request::Ping => Response::Pong,
        Request::Subscribe { topics: t } => { topics.lock().await.extend(t); Response::Ok }
        Request::Unsubscribe { topics: t } => { for x in t { topics.lock().await.remove(&x); } Response::Ok }
        Request::GetSnapshot => match ctx.service.snapshot(ctx.agent_pid, ctx.agent_started_at, llm_relay_core::paths::PROXY_PORT, ctx.keystore_kind).await {
            Ok(s) => Response::Snapshot(s), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::AddGateway(input) => match ctx.service.add_gateway(input).await {
            Ok(_) => Response::Ok, Err(e) => Response::Error { message: e.to_string() },
        },
        Request::UpdateGateway { id, fields } => ok_or_err!(ctx.service.update_gateway(id, fields).await),
        Request::DeleteGateway { id } => ok_or_err!(ctx.service.delete_gateway(id).await),
        Request::SetActive { gateway_id, key_id, models } => ok_or_err!(ctx.service.set_active(gateway_id, key_id, models).await),
        Request::ClearActive => ok_or_err!(ctx.service.clear_active().await),
        Request::SetAutoFailover(b) => ok_or_err!(ctx.service.set_auto_failover(b).await),
        Request::Reorder(ids) => ok_or_err!(ctx.service.reorder(ids).await),
        Request::FetchKeys { gateway_id } => match ctx.service.fetch_keys(gateway_id).await {
            Ok(v) => Response::Keys(v), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::FetchModels { gateway_id, key_id } => match ctx.service.fetch_models(gateway_id, key_id).await {
            Ok(c) => Response::Models(c), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetUsage { range, gateway_id } => match ctx.service.get_usage(range, gateway_id).await {
            Ok(u) => Response::Usage(u), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetTrafficLog { gateway_id } => match ctx.service.get_traffic_log(gateway_id).await {
            Ok(v) => Response::TrafficLog(v), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetSettings => match ctx.service.get_settings().await {
            Ok(s) => Response::Settings(s), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::UpdateSettings(u) => ok_or_err!(ctx.service.update_settings(u).await),
        Request::StartLogin { gateway_id: _ } => Response::Error { message: "not yet implemented (Task 23)".into() },
        Request::CancelLogin { gateway_id: _ } => Response::Ok,
        Request::Shutdown => Response::Ok, // actual shutdown signaled by caller after response flush; see Task 22
    }
}
```

> Note: There is a borrow on `stream` after splitting; we use `split` from `tokio::io` which handles the bound. The `transport::build_listener` returns `interprocess::local_socket::tokio::Listener`; the `accept().await` returns a stream that already implements `AsyncRead + AsyncWrite`. Adjust if the actual API uses different names.

- [ ] **Step 2: Verify**

`cargo build --workspace`. Resolve any minor signature mismatches with `interprocess` types.

Commit:

```bash
git add -A
git commit -m "feat(agent): IPC server with event bus, per-connection routing, topic filtering"
```

### Task 21: Agent main()

**Files:**
- Modify: `crates/llm-relay-agent/src/main.rs`

- [ ] **Step 1: Write main**

Replace `crates/llm-relay-agent/src/main.rs`:

```rust
mod ipc_server;
mod lifecycle;

use anyhow::Result;
use chrono::Utc;
use llm_relay_core::{paths, Database, Service};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    init_log();
    let _guard = lifecycle::LifecycleGuard::acquire()?;
    log::info!("agent starting (pid {})", std::process::id());

    std::fs::create_dir_all(paths::config_dir())?;
    llm_relay_core::keystore::init(&paths::config_dir());

    let db = Arc::new(Database::init(&paths::config_dir())?);
    let bus = ipc_server::EventBus::new();
    let sink: llm_relay_core::SharedEventSink = Arc::new(ipc_server::BusSink { bus: bus.clone() });
    let service = Service::new(db.clone(), sink);

    // Spawn proxy + health
    let s1 = service.clone();
    tokio::spawn(async move { llm_relay_core::proxy_server::start(s1).await });
    let s2 = service.clone();
    tokio::spawn(async move { llm_relay_core::health::health_check_loop(s2).await });

    let shutdown = Arc::new(tokio::sync::Notify::new());
    let ctx = ipc_server::ServerCtx {
        service,
        bus,
        agent_started_at: Utc::now(),
        agent_pid: std::process::id(),
        keystore_kind: llm_relay_core::ipc::protocol::KeystoreKind::System, // refined in Task 22
        shutdown: shutdown.clone(),
    };

    // Listen for SIGTERM / Ctrl-C and trip shutdown.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        log::info!("ctrl_c received");
        sd.notify_one();
    });

    ipc_server::run(&paths::sock_file(), ctx).await?;
    log::info!("agent exiting cleanly");
    Ok(())
}

fn init_log() {
    // Append-only log file at ~/.llm-relay/agent.log.
    use std::io::Write as _;
    let path = paths::log_file();
    let _ = std::fs::create_dir_all(paths::config_dir());
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let target = Box::new(file);
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(target))
            .format(|buf, rec| writeln!(buf, "[{}] {} {}: {}", chrono::Utc::now().to_rfc3339(), rec.level(), rec.target(), rec.args()))
            .init();
    } else {
        env_logger::init();
    }
}
```

Add to `crates/llm-relay-agent/Cargo.toml`:
```toml
env_logger = "0.11"
serde = { workspace = true }
uuid = { workspace = true }
```

- [ ] **Step 2: Verify**

`cargo build --workspace`. Commit:

```bash
git add -A
git commit -m "feat(agent): main entry — start lifecycle, proxy, health, IPC server"
```

### Task 22: Surface keystore backend kind + Shutdown wiring

**Files:**
- Modify: `crates/llm-relay-core/src/keystore.rs` (expose `current_kind()`)
- Modify: `crates/llm-relay-agent/src/main.rs`
- Modify: `crates/llm-relay-agent/src/ipc_server.rs`

- [ ] **Step 1: Expose keystore kind**

Append to `crates/llm-relay-core/src/keystore.rs`:

```rust
use crate::ipc::protocol::KeystoreKind;
use std::sync::OnceLock;

static CURRENT_KIND: OnceLock<KeystoreKind> = OnceLock::new();

pub fn current_kind() -> KeystoreKind {
    *CURRENT_KIND.get().unwrap_or(&KeystoreKind::System)
}

// Edit init() to also store the kind:
//   let _ = CURRENT_KIND.set(if probe ok { KeystoreKind::System } else { KeystoreKind::EncryptedFile });
```

Apply that single-line addition inside the `init` body where the backend is chosen.

- [ ] **Step 2: Use it in agent main**

In `crates/llm-relay-agent/src/main.rs`, replace the hard-coded `KeystoreKind::System` with `llm_relay_core::keystore::current_kind()`.

- [ ] **Step 3: Implement Shutdown handling in dispatch**

In `ipc_server.rs::dispatch`, change the `Request::Shutdown` arm to (a) reply Ok via the channel as today, then (b) trip the shutdown notify. Easiest path: pass `shutdown: Arc<Notify>` into `ConnCtx`:

```rust
struct ConnCtx { /* ... */ shutdown: Arc<tokio::sync::Notify> }
// in clone_for_conn(): shutdown: self.shutdown.clone()
// in dispatch:
Request::Shutdown => {
    let sd = ctx.shutdown.clone();
    tokio::spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(50)).await; sd.notify_one(); });
    Response::Ok
}
```

- [ ] **Step 4: Verify**

Build, then a manual smoke test:

```bash
cargo run -p llm-relay-agent &
sleep 2
ls -l ~/.llm-relay/agent.{pid,sock,lock,log}
# expected: all four files exist
cat ~/.llm-relay/agent.pid
# expected: matches the agent PID
kill %1; wait
ls -l ~/.llm-relay/agent.{pid,sock} 2>&1 | head
# expected: both files removed (Drop ran)
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(agent): expose keystore kind in snapshot; honor Shutdown RPC"
```

---


## Phase 6 — Login state machine on the agent

The agent owns device-code login state per gateway. TUI sends `StartLogin{gateway_id}`,
agent calls `gateway::request_device_code`, returns `LoginInitiated{user_code, verification_uri, expires_in}`
in the Response, then spawns a background polling task that emits `LoginCompleted | LoginFailed | LoginExpired`
events on the bus. TUI sees the events, finalizes the gateway record, and closes the dialog.

### Task 23: Login state machine

**Files:**
- Create: `crates/llm-relay-agent/src/login.rs`
- Modify: `crates/llm-relay-agent/src/lib.rs` (add `pub mod login;`)
- Modify: `crates/llm-relay-agent/src/ipc_server.rs` (route `StartLogin` / `CancelLogin` requests)
- Modify: `crates/llm-relay-core/src/ipc.rs` (add request/response/event variants)
- Test: `crates/llm-relay-agent/tests/login_state_machine.rs`

- [ ] **Step 1: Add IPC variants for login**

In `crates/llm-relay-core/src/ipc.rs`, extend the existing enums:

```rust
// Add to RequestPayload:
StartLogin { gateway_id: Uuid },
CancelLogin { gateway_id: Uuid },

// Add to ResponsePayload:
LoginInitiated {
    gateway_id: Uuid,
    user_code: String,
    verification_uri: String,
    expires_in_secs: u64,
},
LoginCancelled { gateway_id: Uuid },

// Add to Event:
LoginCompleted {
    gateway_id: Uuid,
    user_id: Option<String>,
    user_name: Option<String>,
    session_token: String,
},
LoginFailed { gateway_id: Uuid, message: String },
LoginExpired { gateway_id: Uuid },

// Add to Topic enum:
Login,
```

- [ ] **Step 2: Write the failing test**

In `crates/llm-relay-agent/tests/login_state_machine.rs`:

```rust
use llm_relay_agent::login::{LoginRegistry, LoginOutcome};
use llm_relay_core::ipc::Event;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use uuid::Uuid;

#[tokio::test]
async fn start_login_then_cancel_emits_no_completion() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    // Start a login session manually with a fake poller that never resolves.
    let handle = registry
        .start_with_poller(gid, async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    assert!(handle.is_some(), "first start should succeed");

    // Cancel before completion
    assert!(registry.cancel(gid).await);

    // No event should arrive within a short window.
    let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(result.is_err(), "no event expected after cancel");
}

#[tokio::test]
async fn start_login_twice_for_same_gateway_returns_existing() {
    let (tx, _rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    let h1 = registry
        .start_with_poller(gid, async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    let h2 = registry
        .start_with_poller(gid, async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    assert!(h1.is_some());
    assert!(h2.is_none(), "second start for same gateway must be rejected");
}

#[tokio::test]
async fn poller_completion_emits_login_completed_event() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    registry
        .start_with_poller(gid, async move {
            LoginOutcome::Completed {
                session_token: "tok".into(),
                user_id: Some("u1".into()),
                user_name: Some("alice".into()),
            }
        })
        .await
        .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
        .expect("event arrived")
        .expect("recv ok");
    match evt {
        Event::LoginCompleted { gateway_id, session_token, user_name, .. } => {
            assert_eq!(gateway_id, gid);
            assert_eq!(session_token, "tok");
            assert_eq!(user_name.as_deref(), Some("alice"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn poller_failure_emits_login_failed_event() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    registry
        .start_with_poller(gid, async move {
            LoginOutcome::Failed("access_denied".into())
        })
        .await
        .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
        .expect("event arrived")
        .expect("recv ok");
    match evt {
        Event::LoginFailed { gateway_id, message } => {
            assert_eq!(gateway_id, gid);
            assert_eq!(message, "access_denied");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p llm-relay-agent --test login_state_machine`
Expected: FAIL — `LoginRegistry` and `LoginOutcome` not found.

- [ ] **Step 4: Implement `login.rs`**

Create `crates/llm-relay-agent/src/login.rs`:

```rust
//! Per-gateway login state machine.
//!
//! `LoginRegistry::start` kicks off a device-code flow against a gateway,
//! returns the `DeviceCodeResponse` so the IPC layer can answer `LoginInitiated`,
//! and spawns a background poller that emits `LoginCompleted | LoginFailed |
//! LoginExpired` events on the broadcast bus. Concurrent `start` calls for the
//! same gateway are rejected.

use llm_relay_core::gateway::{self, DeviceCodeResponse};
use llm_relay_core::ipc::Event;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug)]
pub enum LoginOutcome {
    Completed {
        session_token: String,
        user_id: Option<String>,
        user_name: Option<String>,
    },
    Failed(String),
    Expired,
}

struct Session {
    handle: JoinHandle<()>,
}

#[derive(Clone)]
pub struct LoginRegistry {
    inner: Arc<Mutex<HashMap<Uuid, Session>>>,
    events: broadcast::Sender<Event>,
}

impl LoginRegistry {
    pub fn new(events: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            events,
        }
    }

    /// Start a real device-code login against `url`. Returns the device-code
    /// payload so the caller can answer `LoginInitiated` to the requesting
    /// client. Spawns a background poller; emits final event on the bus.
    pub async fn start(
        &self,
        gateway_id: Uuid,
        url: String,
    ) -> Result<DeviceCodeResponse, StartError> {
        // Fast guard: don't even hit the network if a session is already live.
        {
            let map = self.inner.lock().await;
            if map.contains_key(&gateway_id) {
                return Err(StartError::AlreadyRunning);
            }
        }
        let code = gateway::request_device_code(&url)
            .await
            .map_err(|e| StartError::Network(e.to_string()))?;

        let device_code = code.device_code.clone();
        let interval = Duration::from_secs(code.interval.max(1));
        let expires = Duration::from_secs(code.expires_in);
        let events = self.events.clone();
        let url_for_task = url.clone();
        let inner = self.inner.clone();

        let poller = async move {
            poll_until_done(&url_for_task, &device_code, interval, expires).await
        };
        let handle = tokio::spawn(async move {
            let outcome = poller.await;
            let evt = outcome_to_event(gateway_id, outcome);
            // Drop registry entry first so subsequent start calls are allowed.
            let _ = inner.lock().await.remove(&gateway_id);
            let _ = events.send(evt);
        });

        self.inner
            .lock()
            .await
            .insert(gateway_id, Session { handle });

        Ok(code)
    }

    /// Test-only / DI seam: start a login with a hand-crafted poller future.
    /// Returns Some(()) if started, None if a session was already in flight.
    pub async fn start_with_poller<F>(
        &self,
        gateway_id: Uuid,
        poller: F,
    ) -> Option<()>
    where
        F: Future<Output = LoginOutcome> + Send + 'static,
    {
        {
            let map = self.inner.lock().await;
            if map.contains_key(&gateway_id) {
                return None;
            }
        }
        let events = self.events.clone();
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            let outcome = poller.await;
            let evt = outcome_to_event(gateway_id, outcome);
            let _ = inner.lock().await.remove(&gateway_id);
            let _ = events.send(evt);
        });
        self.inner
            .lock()
            .await
            .insert(gateway_id, Session { handle });
        Some(())
    }

    /// Cancel a running login. Returns true if a session was in flight.
    pub async fn cancel(&self, gateway_id: Uuid) -> bool {
        if let Some(session) = self.inner.lock().await.remove(&gateway_id) {
            session.handle.abort();
            true
        } else {
            false
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("a login is already in progress for this gateway")]
    AlreadyRunning,
    #[error("network error: {0}")]
    Network(String),
}

async fn poll_until_done(
    url: &str,
    device_code: &str,
    interval: Duration,
    expires: Duration,
) -> LoginOutcome {
    let deadline = Instant::now() + expires;
    loop {
        if Instant::now() >= deadline {
            return LoginOutcome::Expired;
        }
        tokio::time::sleep(interval).await;
        match gateway::poll_device_code(url, device_code).await {
            Ok(resp) => match resp.status.as_str() {
                "approved" | "completed" | "ok" => {
                    return LoginOutcome::Completed {
                        session_token: resp.session_token.unwrap_or_default(),
                        user_id: resp.user_id,
                        user_name: resp.user_name,
                    };
                }
                "pending" | "authorization_pending" | "slow_down" => continue,
                "denied" | "access_denied" => {
                    return LoginOutcome::Failed("access_denied".into());
                }
                "expired" | "expired_token" => return LoginOutcome::Expired,
                other => return LoginOutcome::Failed(format!("unknown status: {other}")),
            },
            Err(e) => return LoginOutcome::Failed(e.to_string()),
        }
    }
}

fn outcome_to_event(gateway_id: Uuid, outcome: LoginOutcome) -> Event {
    match outcome {
        LoginOutcome::Completed { session_token, user_id, user_name } => {
            Event::LoginCompleted { gateway_id, session_token, user_id, user_name }
        }
        LoginOutcome::Failed(message) => Event::LoginFailed { gateway_id, message },
        LoginOutcome::Expired => Event::LoginExpired { gateway_id },
    }
}
```

- [ ] **Step 5: Wire `login` module into agent**

In `crates/llm-relay-agent/src/lib.rs`, add:

```rust
pub mod login;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p llm-relay-agent --test login_state_machine`
Expected: PASS (4 tests).

- [ ] **Step 7: Route `StartLogin` / `CancelLogin` in `ipc_server.rs`**

Locate the request dispatcher in `crates/llm-relay-agent/src/ipc_server.rs`
(introduced in Phase 5) and add the two new arms. Assume the dispatcher
holds an `Arc<Service>` (for gateway URL lookup) and an `Arc<LoginRegistry>`:

```rust
RequestPayload::StartLogin { gateway_id } => {
    let url = match service.get_gateway_url(gateway_id).await {
        Ok(u) => u,
        Err(e) => return ResponsePayload::Error { message: e.to_string() },
    };
    match login_registry.start(gateway_id, url).await {
        Ok(code) => ResponsePayload::LoginInitiated {
            gateway_id,
            user_code: code.user_code,
            verification_uri: code.verification_uri.unwrap_or_default(),
            expires_in_secs: code.expires_in,
        },
        Err(e) => ResponsePayload::Error { message: e.to_string() },
    }
}
RequestPayload::CancelLogin { gateway_id } => {
    login_registry.cancel(gateway_id).await;
    ResponsePayload::LoginCancelled { gateway_id }
}
```

Note: `verification_uri` is not present in the existing `DeviceCodeResponse`
struct (see `src-tauri/src/gateway.rs:138-146`). The TUI will derive the URL
from the gateway's base URL + `/device/login` (matching the existing browser
page) — so set `verification_uri` to `format!("{base}/device/login")` here
instead of pulling from the response. Adjust accordingly:

```rust
let base = url.trim_end_matches('/');
let verification_uri = format!("{base}/device/login");
// ...
ResponsePayload::LoginInitiated {
    gateway_id,
    user_code: code.user_code,
    verification_uri,
    expires_in_secs: code.expires_in,
}
```

- [ ] **Step 8: Construct `LoginRegistry` in agent main**

In `crates/llm-relay-agent/src/main.rs` (created in Phase 5), after the
`EventBus` is built, instantiate the registry with the bus's broadcast
sender and pass it into the IPC server:

```rust
let login_registry = std::sync::Arc::new(
    llm_relay_agent::login::LoginRegistry::new(event_bus.sender())
);
ipc_server::run(socket_path, service.clone(), event_bus.clone(), login_registry).await?;
```

- [ ] **Step 9: Cargo check**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-agent/src/login.rs \
        crates/llm-relay-agent/src/lib.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-agent/src/main.rs \
        crates/llm-relay-agent/tests/login_state_machine.rs
git commit -m "feat(agent): per-gateway device-code login state machine"
```

---


## Phase 7 — TUI process: spawn helpers, IPC client, attach-or-spawn

The TUI binary's first job is to either attach to a running agent or fork a
fresh detached one. After connecting, it owns an IPC client that:
- correlates Responses to Requests via `request_id`
- broadcasts Events to UI subscribers
- reconnects if the agent goes away mid-session (with backoff)

### Task 24: Cross-platform detached spawn

**Files:**
- Create: `crates/llm-relay-tui/src/spawn.rs`
- Modify: `crates/llm-relay-tui/src/lib.rs` (add `pub mod spawn;`)
- Test: `crates/llm-relay-tui/tests/spawn.rs`

- [ ] **Step 1: Write the failing test**

In `crates/llm-relay-tui/tests/spawn.rs`:

```rust
use llm_relay_tui::spawn;
use std::time::Duration;

#[test]
fn spawn_detached_returns_pid_and_child_outlives_parent_check() {
    // Spawn a long-running, harmless command (`sleep 5` on unix, `timeout 5` on windows).
    #[cfg(unix)]
    let (cmd, args): (&str, &[&str]) = ("sleep", &["5"]);
    #[cfg(windows)]
    let (cmd, args): (&str, &[&str]) = ("cmd", &["/C", "ping -n 5 127.0.0.1 >NUL"]);

    let pid = spawn::spawn_detached(cmd, args).expect("spawn ok");
    assert!(pid > 0);
    // Give the child a moment to register with the OS.
    std::thread::sleep(Duration::from_millis(50));
    assert!(spawn::process_alive(pid), "child should be alive");

    // Clean up
    #[cfg(unix)]
    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    #[cfg(windows)] {
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h != 0 { TerminateProcess(h, 0); }
        }
    }
}
```

- [ ] **Step 2: Run test (will fail to compile)**

Run: `cargo test -p llm-relay-tui --test spawn`
Expected: FAIL — `spawn::spawn_detached` and `spawn::process_alive` not found.

- [ ] **Step 3: Implement `spawn.rs`**

Create `crates/llm-relay-tui/src/spawn.rs`:

```rust
//! Spawn the agent binary as a detached background process.
//!
//! Unix: double-fork via `daemonize`-style call with `setsid` and stdio
//! redirected to /dev/null so the child outlives the parent shell session.
//!
//! Windows: `CreateProcessW` with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
//! flags so the child does not inherit the parent's console.

use std::io;

#[cfg(unix)]
pub fn spawn_detached(cmd: &str, args: &[&str]) -> io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = unsafe {
        Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                // Become a new session leader so we are detached from the
                // controlling terminal. Errors here propagate to the caller.
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()?
    };
    let pid = child.id();
    // Don't wait — we want a true detach. Caller relies on PID file written by
    // the agent itself for liveness tracking, not on our `Child` handle.
    std::mem::forget(child);
    Ok(pid)
}

#[cfg(windows)]
pub fn spawn_detached(cmd: &str, args: &[&str]) -> io::Result<u32> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // Constants from windows-sys::Win32::System::Threading
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()?;
    let pid = child.id();
    std::mem::forget(child);
    Ok(pid)
}

/// Probe whether a process with the given pid is still alive.
/// Implementation lives in `llm-relay-core::process` — re-exported for
/// convenience so `spawn` consumers don't need an extra import.
pub fn process_alive(pid: u32) -> bool {
    llm_relay_core::process::is_alive(pid)
}
```

Note: `llm_relay_core::process::is_alive` was created in Phase 5 (Task 19,
lifecycle guard). If not yet present, add it now in
`crates/llm-relay-core/src/process.rs`:

```rust
#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code as i32 == STILL_ACTIVE.0
    }
}
```

- [ ] **Step 4: Add deps if missing**

In `crates/llm-relay-tui/Cargo.toml`:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.52", features = ["Win32_System_Threading", "Win32_Foundation"] }
```

- [ ] **Step 5: Run test to verify pass**

Run: `cargo test -p llm-relay-tui --test spawn`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-relay-tui/src/spawn.rs \
        crates/llm-relay-tui/src/lib.rs \
        crates/llm-relay-tui/Cargo.toml \
        crates/llm-relay-tui/tests/spawn.rs \
        crates/llm-relay-core/src/process.rs
git commit -m "feat(tui): cross-platform detached agent spawn"
```

---

### Task 25: IPC client (request_id correlation + event broadcast)

**Files:**
- Create: `crates/llm-relay-tui/src/ipc_client.rs`
- Modify: `crates/llm-relay-tui/src/lib.rs` (add `pub mod ipc_client;`)
- Test: `crates/llm-relay-tui/tests/ipc_client_roundtrip.rs`

- [ ] **Step 1: Write the failing test**

In `crates/llm-relay-tui/tests/ipc_client_roundtrip.rs`:

```rust
use llm_relay_core::ipc::{
    ClientFrame, Event, RequestPayload, ResponsePayload, ServerFrame,
};
use llm_relay_core::ipc_codec::{read_frame, write_frame};
use llm_relay_tui::ipc_client::IpcClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

#[tokio::test]
async fn ping_returns_pong_via_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Fake agent: one accept, echo Ping → Pong with the right request_id
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let frame: ClientFrame = read_frame(&mut stream).await.unwrap();
        assert!(matches!(frame.payload, RequestPayload::Ping));
        let resp = ServerFrame::Response {
            request_id: frame.request_id,
            payload: ResponsePayload::Pong,
        };
        write_frame(&mut stream, &resp).await.unwrap();
        // Hold the connection so the client doesn't see EOF before reading.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let client = IpcClient::connect(&sock).await.unwrap();
    let resp = client.request(RequestPayload::Ping).await.unwrap();
    assert!(matches!(resp, ResponsePayload::Pong));
    server.await.unwrap();
}

#[tokio::test]
async fn events_are_broadcast_to_subscribers() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Push a HealthChanged event spontaneously
        let evt = ServerFrame::Event(Event::HealthChanged {
            gateway_id: uuid::Uuid::nil(),
            healthy: true,
            latency_ms: Some(42),
        });
        write_frame(&mut stream, &evt).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let client = IpcClient::connect(&sock).await.unwrap();
    let mut sub = client.subscribe();
    let evt = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await.unwrap().unwrap();
    match evt {
        Event::HealthChanged { latency_ms, .. } => assert_eq!(latency_ms, Some(42)),
        other => panic!("unexpected event: {other:?}"),
    }
    server.await.unwrap();
}
```

- [ ] **Step 2: Run test (will fail)**

Run: `cargo test -p llm-relay-tui --test ipc_client_roundtrip`
Expected: FAIL — `IpcClient` not found.

- [ ] **Step 3: Implement `ipc_client.rs`**

Create `crates/llm-relay-tui/src/ipc_client.rs`:

```rust
//! IPC client used by the TUI. Owns one connection to the agent.
//!
//! Architecture:
//! - A reader task pulls `ServerFrame`s off the socket, routes `Response`s to
//!   the matching pending oneshot, and broadcasts `Event`s to subscribers.
//! - A writer mutex serializes `ClientFrame` writes.
//! - `request(payload)` allocates a `request_id`, registers a oneshot,
//!   writes the frame, awaits the oneshot. Times out after 30s.

use llm_relay_core::ipc::{ClientFrame, Event, RequestPayload, ResponsePayload, ServerFrame};
use llm_relay_core::ipc_codec::{read_frame, write_frame};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot, Mutex};
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("connection closed")]
    Closed,
    #[error("request timed out")]
    Timeout,
    #[error("agent error: {0}")]
    Agent(String),
}

type PendingMap = Arc<Mutex<HashMap<Uuid, oneshot::Sender<ResponsePayload>>>>;

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = interprocess::os::windows::named_pipe::tokio::DuplexPipeStream<
    interprocess::os::windows::named_pipe::pipe_mode::Bytes,
>;

pub struct IpcClient {
    writer: Arc<Mutex<tokio::io::WriteHalf<Stream>>>,
    pending: PendingMap,
    events_tx: broadcast::Sender<Event>,
}

impl IpcClient {
    pub async fn connect(socket: &Path) -> Result<Arc<Self>, ClientError> {
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(socket).await?;
        #[cfg(windows)]
        let stream = {
            let s = socket.to_string_lossy().to_string();
            interprocess::os::windows::named_pipe::tokio::DuplexPipeStream::<
                interprocess::os::windows::named_pipe::pipe_mode::Bytes,
            >::connect(s).await?
        };

        let (read_half, write_half) = tokio::io::split(stream);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _) = broadcast::channel(256);

        let client = Arc::new(Self {
            writer: Arc::new(Mutex::new(write_half)),
            pending: pending.clone(),
            events_tx: events_tx.clone(),
        });

        tokio::spawn(reader_loop(read_half, pending, events_tx));

        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    pub async fn request(
        &self,
        payload: RequestPayload,
    ) -> Result<ResponsePayload, ClientError> {
        let (tx, rx) = oneshot::channel();
        let request_id = Uuid::new_v4();
        self.pending.lock().await.insert(request_id, tx);

        let frame = ClientFrame { request_id, payload };
        {
            let mut w = self.writer.lock().await;
            write_frame(&mut *w, &frame).await?;
        }

        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(payload)) => match payload {
                ResponsePayload::Error { message } => Err(ClientError::Agent(message)),
                other => Ok(other),
            },
            Ok(Err(_)) => Err(ClientError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(ClientError::Timeout)
            }
        }
    }
}

async fn reader_loop(
    mut read: tokio::io::ReadHalf<Stream>,
    pending: PendingMap,
    events: broadcast::Sender<Event>,
) {
    loop {
        match read_frame::<_, ServerFrame>(&mut read).await {
            Ok(ServerFrame::Response { request_id, payload }) => {
                if let Some(tx) = pending.lock().await.remove(&request_id) {
                    let _ = tx.send(payload);
                }
            }
            Ok(ServerFrame::Event(evt)) => {
                let _ = events.send(evt);
            }
            Err(_) => {
                // Drain pending with Closed errors via dropping senders.
                pending.lock().await.clear();
                break;
            }
        }
    }
}
```

Note: assumes `read_frame` accepts an `AsyncRead` and `write_frame` accepts
an `AsyncWrite` (defined in Phase 3 Task 12). If their signatures take a
concrete `Stream`, lift them to generic now — they already need to handle
both `UnixStream` and the Windows pipe.

- [ ] **Step 4: Run test to verify pass**

Run: `cargo test -p llm-relay-tui --test ipc_client_roundtrip`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-tui/src/ipc_client.rs \
        crates/llm-relay-tui/src/lib.rs \
        crates/llm-relay-tui/tests/ipc_client_roundtrip.rs
git commit -m "feat(tui): IPC client with request_id correlation and event broadcast"
```

---

### Task 26: Attach-or-spawn flow

**Files:**
- Create: `crates/llm-relay-tui/src/bootstrap.rs`
- Modify: `crates/llm-relay-tui/src/lib.rs` (add `pub mod bootstrap;`)
- Test: `crates/llm-relay-tui/tests/bootstrap.rs`

- [ ] **Step 1: Write the failing test**

In `crates/llm-relay-tui/tests/bootstrap.rs`:

```rust
use llm_relay_tui::bootstrap::{ensure_agent, EnsureMode};

#[tokio::test]
async fn returns_attached_when_socket_already_exists_and_responds() {
    // Set up a fake agent listening on a temp socket.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let server = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            use llm_relay_core::ipc::*;
            use llm_relay_core::ipc_codec::*;
            let frame: ClientFrame = read_frame(&mut stream).await.unwrap();
            let resp = ServerFrame::Response {
                request_id: frame.request_id,
                payload: ResponsePayload::Pong,
            };
            write_frame(&mut stream, &resp).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let result = ensure_agent(&sock, EnsureMode::AttachOnly).await.unwrap();
    assert!(matches!(result, llm_relay_tui::bootstrap::AgentHandle::Attached(_)));
    server.await.unwrap();
}

#[tokio::test]
async fn fails_attach_only_when_no_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("nope.sock");
    let result = ensure_agent(&sock, EnsureMode::AttachOnly).await;
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run test (will fail)**

Run: `cargo test -p llm-relay-tui --test bootstrap`
Expected: FAIL — `bootstrap` module missing.

- [ ] **Step 3: Implement `bootstrap.rs`**

Create `crates/llm-relay-tui/src/bootstrap.rs`:

```rust
//! Decide whether to attach to a running agent or spawn a fresh one.
//!
//! Order of operations:
//!  1. If the socket exists, try a `Ping` over it. On success → Attached.
//!  2. Otherwise, if `mode == AttachOnly`, return `NoAgent`.
//!  3. Otherwise: spawn the agent binary detached, then poll the socket
//!     for up to 5 seconds, retrying `Ping` until it succeeds.

use crate::ipc_client::IpcClient;
use crate::spawn;
use llm_relay_core::ipc::RequestPayload;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum EnsureMode {
    /// Attach if running; spawn a new agent if not.
    AttachOrSpawn,
    /// Attach only — fail if no agent is running.
    AttachOnly,
}

pub enum AgentHandle {
    /// Connected to a pre-existing agent.
    Attached(Arc<IpcClient>),
    /// Spawned a new agent and connected.
    Spawned { client: Arc<IpcClient>, pid: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("no agent running and AttachOnly was requested")]
    NoAgent,
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("agent did not become ready within timeout")]
    Timeout,
    #[error(transparent)]
    Client(#[from] crate::ipc_client::ClientError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn ensure_agent(
    socket: &Path,
    mode: EnsureMode,
) -> Result<AgentHandle, BootstrapError> {
    if socket.exists() {
        if let Ok(client) = IpcClient::connect(socket).await {
            if client.request(RequestPayload::Ping).await.is_ok() {
                return Ok(AgentHandle::Attached(client));
            }
        }
        // Stale socket: remove and fall through to spawn (or error if AttachOnly).
        let _ = std::fs::remove_file(socket);
    }

    match mode {
        EnsureMode::AttachOnly => Err(BootstrapError::NoAgent),
        EnsureMode::AttachOrSpawn => {
            let agent_bin = locate_agent_binary()?;
            let pid = spawn::spawn_detached(
                agent_bin.to_str().expect("utf-8 path"),
                &[],
            )
            .map_err(|e| BootstrapError::Spawn(e.to_string()))?;

            // Wait up to 5s for the agent to bind its socket and answer Ping.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if Instant::now() >= deadline {
                    return Err(BootstrapError::Timeout);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !socket.exists() {
                    continue;
                }
                if let Ok(client) = IpcClient::connect(socket).await {
                    if client.request(RequestPayload::Ping).await.is_ok() {
                        return Ok(AgentHandle::Spawned { client, pid });
                    }
                }
            }
        }
    }
}

/// Find the agent binary alongside the current TUI executable.
/// Cargo lays them out as `target/<profile>/llm-relay-agent` and
/// `target/<profile>/llm-relay-tui`, so a sibling lookup works.
fn locate_agent_binary() -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let dir = me
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir for current_exe"))?;
    let name = if cfg!(windows) {
        "llm-relay-agent.exe"
    } else {
        "llm-relay-agent"
    };
    let candidate = dir.join(name);
    if !candidate.exists() {
        return Err(std::io::Error::other(format!(
            "agent binary not found at {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}
```

- [ ] **Step 4: Run test to verify pass**

Run: `cargo test -p llm-relay-tui --test bootstrap`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-tui/src/bootstrap.rs \
        crates/llm-relay-tui/src/lib.rs \
        crates/llm-relay-tui/tests/bootstrap.rs
git commit -m "feat(tui): attach-or-spawn agent bootstrap"
```

---


## Phase 8 — TUI app shell, tab routing, Gateways tab

The shell is the central event loop: a `crossterm` reader feeds keys into the
state, the `IpcClient` event subscriber feeds events into the state, and a
`ratatui` render pass paints the current tab. Tabs: Gateways / Usage / Errors
/ Settings. This phase wires the shell + ships the Gateways tab.

### Task 27: TUI shell, terminal lifecycle, tab router

**Files:**
- Create: `crates/llm-relay-tui/src/app/mod.rs`
- Create: `crates/llm-relay-tui/src/app/state.rs`
- Create: `crates/llm-relay-tui/src/app/event.rs`
- Modify: `crates/llm-relay-tui/src/main.rs`
- Test: `crates/llm-relay-tui/tests/app_state.rs`

- [ ] **Step 1: Write the failing test**

In `crates/llm-relay-tui/tests/app_state.rs`:

```rust
use llm_relay_tui::app::state::{AppState, Tab};
use llm_relay_tui::app::event::AppEvent;

#[test]
fn tab_navigation_cycles_through_all_tabs() {
    let mut s = AppState::new();
    assert_eq!(s.active_tab, Tab::Gateways);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Usage);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Errors);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Settings);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Gateways);
}

#[test]
fn quit_event_sets_should_quit_flag() {
    let mut s = AppState::new();
    assert!(!s.should_quit);
    s.handle(AppEvent::Quit);
    assert!(s.should_quit);
}
```

- [ ] **Step 2: Run test (will fail)**

Run: `cargo test -p llm-relay-tui --test app_state`
Expected: FAIL — `app::state` and `app::event` not found.

- [ ] **Step 3: Implement `event.rs`**

Create `crates/llm-relay-tui/src/app/event.rs`:

```rust
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
}
```

- [ ] **Step 4: Implement `state.rs`**

Create `crates/llm-relay-tui/src/app/state.rs`:

```rust
//! Pure state. No I/O, no ratatui, no crossterm.
//! Everything that mutates state goes through `handle(AppEvent)` so we can
//! unit-test behavior without a terminal.

use crate::app::event::AppEvent;
use llm_relay_core::ipc::Event as IpcEvent;
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
pub struct AppState {
    pub active_tab: Tab,
    pub should_quit: bool,
    pub gateways: Vec<GatewayRow>,
    pub gateway_index: HashMap<Uuid, usize>,
    pub selected_row: usize,
    pub status_message: Option<String>,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Gateways
    }
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
            AppEvent::Char(_) => { /* tab-specific handlers fill this in */ }
            AppEvent::Refresh => { /* triggers an IPC fetch in the loop */ }
            AppEvent::Ipc(evt) => self.apply_ipc(evt),
        }
    }

    fn apply_ipc(&mut self, evt: IpcEvent) {
        match evt {
            IpcEvent::HealthChanged { gateway_id, healthy, latency_ms } => {
                if let Some(&idx) = self.gateway_index.get(&gateway_id) {
                    if let Some(row) = self.gateways.get_mut(idx) {
                        row.healthy = Some(healthy);
                        row.latency_ms = latency_ms;
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
}
```

- [ ] **Step 5: Implement `app/mod.rs`**

Create `crates/llm-relay-tui/src/app/mod.rs`:

```rust
pub mod event;
pub mod state;
pub mod terminal;
pub mod loop_;
```

- [ ] **Step 6: Run state test, verify pass**

Run: `cargo test -p llm-relay-tui --test app_state`
Expected: PASS (2 tests).

- [ ] **Step 7: Implement `terminal.rs` (raw mode + alt screen lifecycle)**

Create `crates/llm-relay-tui/src/app/terminal.rs`:

```rust
//! Owns terminal init/teardown so we never leak the alternate screen on panic.

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub struct TermGuard {
    pub terminal: Tui,
}

impl TermGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        // Install a panic hook that always restores the terminal.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            prev(info);
        }));
        Ok(Self { terminal })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
```

- [ ] **Step 8: Implement `loop_.rs` (event-driven main loop)**

Create `crates/llm-relay-tui/src/app/loop_.rs`:

```rust
//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.

use crate::app::{event::AppEvent, state::AppState, terminal::Tui};
use crate::ipc_client::IpcClient;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub async fn run(mut term: Tui, client: Arc<IpcClient>) -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Spawn key reader.
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(CtEvent::Key(k)) = event::read() {
                    if k.kind != KeyEventKind::Press { continue; }
                    let app_evt = match k.code {
                        KeyCode::Char('q') => AppEvent::Quit,
                        KeyCode::Tab => AppEvent::NextTab,
                        KeyCode::BackTab => AppEvent::PrevTab,
                        KeyCode::Up => AppEvent::Up,
                        KeyCode::Down => AppEvent::Down,
                        KeyCode::Enter => AppEvent::Enter,
                        KeyCode::Esc => AppEvent::Esc,
                        KeyCode::Char('r') => AppEvent::Refresh,
                        KeyCode::Char(c) => AppEvent::Char(c),
                        _ => continue,
                    };
                    if tx.send(app_evt).is_err() { break; }
                }
            }
        });
    }

    // Spawn IPC event forwarder.
    {
        let tx = tx.clone();
        let mut sub = client.subscribe();
        tokio::spawn(async move {
            while let Ok(evt) = sub.recv().await {
                if tx.send(AppEvent::Ipc(evt)).is_err() { break; }
            }
        });
    }

    let mut state = AppState::new();

    // Initial render
    term.draw(|f| crate::view::render(f, &state))?;

    while let Some(evt) = rx.recv().await {
        state.handle(evt);
        term.draw(|f| crate::view::render(f, &state))?;
        if state.should_quit {
            break;
        }
    }
    Ok(())
}
```

- [ ] **Step 9: Stub `view` module so the build links**

Create `crates/llm-relay-tui/src/view/mod.rs`:

```rust
//! Rendering. Implemented in Task 28 (Gateways tab).
use crate::app::state::AppState;
use ratatui::Frame;

pub fn render(_frame: &mut Frame, _state: &AppState) {
    // Filled in by Task 28.
}
```

Add `pub mod view;` to `crates/llm-relay-tui/src/lib.rs`.

- [ ] **Step 10: Wire `main.rs`**

Replace `crates/llm-relay-tui/src/main.rs`:

```rust
use llm_relay_tui::app::{loop_, terminal::TermGuard};
use llm_relay_tui::bootstrap::{ensure_agent, AgentHandle, EnsureMode};
use llm_relay_core::paths;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = paths::ipc_socket_path()?;
    let handle = ensure_agent(&socket, EnsureMode::AttachOrSpawn).await?;
    let client = match handle {
        AgentHandle::Attached(c) => c,
        AgentHandle::Spawned { client, .. } => client,
    };

    let mut guard = TermGuard::enter()?;
    let term = std::mem::replace(&mut guard.terminal, ratatui::Terminal::new(
        ratatui::backend::CrosstermBackend::new(std::io::stdout())
    )?);
    loop_::run(term, client).await?;
    Ok(())
}
```

- [ ] **Step 11: Cargo check**

Run: `cargo check -p llm-relay-tui`
Expected: clean.

- [ ] **Step 12: Commit**

```bash
git add crates/llm-relay-tui/src/
git commit -m "feat(tui): app shell, terminal lifecycle, tab routing"
```

---

### Task 28: Gateways tab — list, health icons, star, expand

**Files:**
- Create: `crates/llm-relay-tui/src/view/gateways.rs`
- Modify: `crates/llm-relay-tui/src/view/mod.rs`
- Modify: `crates/llm-relay-tui/src/app/state.rs` (toggle_star, ApplyConfig wiring)
- Test: `crates/llm-relay-tui/tests/gateways_view.rs`

- [ ] **Step 1: Write the failing test (state-level)**

In `crates/llm-relay-tui/tests/gateways_view.rs`:

```rust
use llm_relay_tui::app::state::{AppState, GatewayRow};
use llm_relay_tui::app::event::AppEvent;
use uuid::Uuid;

fn make_state(n: usize) -> AppState {
    let mut s = AppState::new();
    let rows = (0..n).map(|i| GatewayRow {
        id: Uuid::new_v4(),
        name: format!("gw-{i}"),
        url: format!("http://example.com/{i}"),
        healthy: None,
        latency_ms: None,
        starred: false,
        expanded: false,
    }).collect();
    s.replace_gateways(rows);
    s
}

#[test]
fn pressing_s_toggles_star_on_selected_row() {
    let mut s = make_state(3);
    assert!(!s.gateways[0].starred);
    s.handle(AppEvent::Char('s'));
    assert!(s.gateways[0].starred);
    s.handle(AppEvent::Char('s'));
    assert!(!s.gateways[0].starred);
}

#[test]
fn down_then_enter_expands_second_row() {
    let mut s = make_state(3);
    s.handle(AppEvent::Down);
    s.handle(AppEvent::Enter);
    assert!(!s.gateways[0].expanded);
    assert!(s.gateways[1].expanded);
    assert!(!s.gateways[2].expanded);
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test -p llm-relay-tui --test gateways_view`
Expected: FAIL — `Char('s')` is currently a no-op.

- [ ] **Step 3: Wire `Char('s')` to toggle star on selected row**

In `crates/llm-relay-tui/src/app/state.rs`, replace the `AppEvent::Char(_)` arm:

```rust
AppEvent::Char(c) => match c {
    's' => {
        if let Some(row) = self.gateways.get_mut(self.selected_row) {
            row.starred = !row.starred;
        }
    }
    _ => {}
},
```

- [ ] **Step 4: Run, expect pass**

Run: `cargo test -p llm-relay-tui --test gateways_view`
Expected: PASS (2 tests).

- [ ] **Step 5: Implement Gateways view**

Create `crates/llm-relay-tui/src/view/gateways.rs`:

```rust
use crate::app::state::{AppState, GatewayRow};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let items: Vec<ListItem> = state
        .gateways
        .iter()
        .enumerate()
        .map(|(i, row)| row_to_item(i, row, i == state.selected_row))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Gateways"))
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_row));
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    let hint = Paragraph::new(
        "↑/↓ select  Enter expand  s star  a add  e edit  l login  d delete  r refresh  Tab next  q quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}

fn row_to_item(_i: usize, row: &GatewayRow, _selected: bool) -> ListItem<'_> {
    let icon = match row.healthy {
        Some(true) => Span::styled("●", Style::default().fg(Color::Green)),
        Some(false) => Span::styled("●", Style::default().fg(Color::Red)),
        None => Span::styled("●", Style::default().fg(Color::DarkGray)),
    };
    let star = if row.starred { "★ " } else { "  " };
    let latency = row
        .latency_ms
        .map(|ms| format!(" {ms}ms"))
        .unwrap_or_default();
    let header = Line::from(vec![
        Span::raw(star),
        icon,
        Span::raw("  "),
        Span::styled(&row.name, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(&row.url, Style::default().fg(Color::DarkGray)),
        Span::raw(latency),
    ]);
    if row.expanded {
        let detail = Line::from(vec![Span::styled(
            format!("    id={}", row.id),
            Style::default().fg(Color::DarkGray),
        )]);
        ListItem::new(vec![header, detail])
    } else {
        ListItem::new(vec![header])
    }
}
```

- [ ] **Step 6: Wire tabs into `view/mod.rs`**

Replace `crates/llm-relay-tui/src/view/mod.rs`:

```rust
pub mod gateways;

use crate::app::state::{AppState, Tab};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::Frame;

pub fn render(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(frame.size());

    let titles: Vec<Line> = ["Gateways", "Usage", "Errors", "Settings"]
        .iter()
        .copied()
        .map(Line::from)
        .collect();
    let selected = match state.active_tab {
        Tab::Gateways => 0,
        Tab::Usage => 1,
        Tab::Errors => 2,
        Tab::Settings => 3,
    };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("LLM Relay"))
        .select(selected)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    frame.render_widget(tabs, chunks[0]);

    match state.active_tab {
        Tab::Gateways => gateways::render(frame, chunks[1], state),
        Tab::Usage | Tab::Errors | Tab::Settings => {
            let p = Paragraph::new("(coming in next tasks)")
                .block(Block::default().borders(Borders::ALL));
            frame.render_widget(p, chunks[1]);
        }
    }
}
```

- [ ] **Step 7: Cargo check, then a smoke run**

Run: `cargo check -p llm-relay-tui`
Expected: clean.

Manual smoke (optional, requires agent built): `cargo run -p llm-relay-tui`,
verify the Gateways tab renders, tab navigation works, q quits cleanly with
no leftover terminal corruption.

- [ ] **Step 8: Commit**

```bash
git add crates/llm-relay-tui/src/view/ \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/tests/gateways_view.rs
git commit -m "feat(tui): gateways tab with health icons, star, expand"
```

---

### Task 29: Initial gateway load + apply on startup

**Files:**
- Modify: `crates/llm-relay-tui/src/app/loop_.rs`
- Modify: `crates/llm-relay-core/src/ipc.rs` (add `ListGateways` request if missing)

- [ ] **Step 1: Add `ListGateways` request/response if not present**

In `crates/llm-relay-core/src/ipc.rs`:

```rust
// In RequestPayload:
ListGateways,

// In ResponsePayload:
GatewayList { gateways: Vec<GatewaySummary> },

// New struct:
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewaySummary {
    pub id: uuid::Uuid,
    pub name: String,
    pub url: String,
    pub starred: bool,
    pub healthy: Option<bool>,
    pub latency_ms: Option<i64>,
}
```

- [ ] **Step 2: Wire `ListGateways` in agent IPC server**

In `crates/llm-relay-agent/src/ipc_server.rs` request dispatcher, add:

```rust
RequestPayload::ListGateways => {
    match service.list_gateways().await {
        Ok(rows) => ResponsePayload::GatewayList {
            gateways: rows.into_iter().map(Into::into).collect(),
        },
        Err(e) => ResponsePayload::Error { message: e.to_string() },
    }
}
```

`Service::list_gateways` already exists from Phase 4 (Task 15). Add a
`From<service::Gateway> for GatewaySummary` impl in the same file or
in `core/src/ipc.rs` next to the type.

- [ ] **Step 3: Issue `ListGateways` on startup, populate state**

In `crates/llm-relay-tui/src/app/loop_.rs`, before the render+event loop:

```rust
use llm_relay_core::ipc::{RequestPayload, ResponsePayload};
use crate::app::state::GatewayRow;

if let Ok(ResponsePayload::GatewayList { gateways }) =
    client.request(RequestPayload::ListGateways).await
{
    let rows: Vec<GatewayRow> = gateways
        .into_iter()
        .map(|g| GatewayRow {
            id: g.id,
            name: g.name,
            url: g.url,
            healthy: g.healthy,
            latency_ms: g.latency_ms,
            starred: g.starred,
            expanded: false,
        })
        .collect();
    state.replace_gateways(rows);
}
```

(`state` must be declared before this block — move its `let mut state =
AppState::new();` line above this fetch.)

Also issue `ListGateways` again on `AppEvent::Refresh` — handle it in the
event match arm rather than `AppState::handle`, since it requires the
async client:

```rust
AppEvent::Refresh => {
    if let Ok(ResponsePayload::GatewayList { gateways }) =
        client.request(RequestPayload::ListGateways).await
    {
        let rows: Vec<GatewayRow> = gateways.into_iter().map(/* same map */).collect();
        state.replace_gateways(rows);
    }
}
_ => state.handle(evt.clone()),
```

(Restructure the match so `Refresh` short-circuits before falling through
to `state.handle`.)

- [ ] **Step 4: Cargo check**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-tui/src/app/loop_.rs
git commit -m "feat(tui): initial gateway list on startup + manual refresh"
```

---


## Phase 9 — Usage tab, Errors tab, Settings tab

These three tabs are read-only views over agent-side data. Each follows the
same pattern: TUI sends a Request on tab activation (and on `r` to refresh),
agent answers with a Response containing rows, TUI renders a table.

### Task 30: Usage tab

**Files:**
- Create: `crates/llm-relay-tui/src/view/usage.rs`
- Modify: `crates/llm-relay-tui/src/view/mod.rs` (route `Tab::Usage`)
- Modify: `crates/llm-relay-tui/src/app/state.rs` (UsageState, range filter)
- Modify: `crates/llm-relay-core/src/ipc.rs` (`GetUsage` request)
- Modify: `crates/llm-relay-agent/src/ipc_server.rs` (handler)
- Test: `crates/llm-relay-tui/tests/usage_state.rs`

- [ ] **Step 1: Add IPC types**

In `crates/llm-relay-core/src/ipc.rs`:

```rust
// In RequestPayload:
GetUsage { range: UsageRange },

// In ResponsePayload:
UsageRows { rows: Vec<UsageRow> },

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum UsageRange { Today, Last7Days, Last30Days, AllTime }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageRow {
    pub gateway_id: uuid::Uuid,
    pub gateway_name: String,
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}
```

- [ ] **Step 2: Service method**

Add `Service::get_usage(range)` in `crates/llm-relay-core/src/service.rs`,
delegating to whatever DAO the GUI already uses for the Usage panel
(check `src-tauri/src/lib.rs` for the existing Tauri command — wrap the
same query). Return `Vec<UsageRow>`.

- [ ] **Step 3: Wire agent handler**

In `crates/llm-relay-agent/src/ipc_server.rs`:

```rust
RequestPayload::GetUsage { range } => match service.get_usage(range).await {
    Ok(rows) => ResponsePayload::UsageRows { rows },
    Err(e) => ResponsePayload::Error { message: e.to_string() },
},
```

- [ ] **Step 4: Add UsageState + tests**

In `crates/llm-relay-tui/src/app/state.rs`, append:

```rust
use llm_relay_core::ipc::{UsageRange, UsageRow};

#[derive(Debug, Default)]
pub struct UsageState {
    pub range: UsageRange,
    pub rows: Vec<UsageRow>,
    pub selected: usize,
}

impl Default for UsageRange {
    fn default() -> Self { UsageRange::Today }
}

impl AppState {
    // hook these into existing handle()
    pub fn cycle_usage_range(&mut self) {
        self.usage.range = match self.usage.range {
            UsageRange::Today => UsageRange::Last7Days,
            UsageRange::Last7Days => UsageRange::Last30Days,
            UsageRange::Last30Days => UsageRange::AllTime,
            UsageRange::AllTime => UsageRange::Today,
        };
    }
}
```

Add `pub usage: UsageState,` to the `AppState` struct.

In `crates/llm-relay-tui/tests/usage_state.rs`:

```rust
use llm_relay_tui::app::state::AppState;
use llm_relay_core::ipc::UsageRange;

#[test]
fn cycle_range_visits_all_then_wraps() {
    let mut s = AppState::new();
    assert_eq!(s.usage.range, UsageRange::Today);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Last7Days);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Last30Days);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::AllTime);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Today);
}
```

Run: `cargo test -p llm-relay-tui --test usage_state` → PASS.

- [ ] **Step 5: Implement usage view**

Create `crates/llm-relay-tui/src/view/usage.rs`:

```rust
use crate::app::state::AppState;
use llm_relay_core::ipc::UsageRange;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let range_label = match state.usage.range {
        UsageRange::Today => "Today",
        UsageRange::Last7Days => "Last 7 days",
        UsageRange::Last30Days => "Last 30 days",
        UsageRange::AllTime => "All time",
    };
    let header_p = Paragraph::new(format!("Range: {range_label}  (press 'p' to cycle)"))
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(header_p, chunks[0]);

    let header = Row::new(vec!["Gateway", "Model", "Reqs", "In", "Out", "Cost ($)"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    let rows = state.usage.rows.iter().map(|r| {
        Row::new(vec![
            Cell::from(r.gateway_name.clone()),
            Cell::from(r.model.clone()),
            Cell::from(r.requests.to_string()),
            Cell::from(r.input_tokens.to_string()),
            Cell::from(r.output_tokens.to_string()),
            Cell::from(format!("{:.4}", r.cost_usd)),
        ])
    });
    let widths = [
        Constraint::Percentage(22),
        Constraint::Percentage(28),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Usage"))
        .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut ts = TableState::default();
    ts.select(Some(state.usage.selected));
    frame.render_stateful_widget(table, chunks[1], &mut ts);

    let hint = Paragraph::new("p cycle range  r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[2]);
}
```

Wire `pub mod usage;` and `Tab::Usage => usage::render(...)` in
`view/mod.rs`.

- [ ] **Step 6: Wire `p` key + on-activation fetch in loop_**

In `crates/llm-relay-tui/src/app/state.rs`, update the `Char` arm:

```rust
AppEvent::Char(c) => match c {
    's' => { /* existing */ }
    'p' if self.active_tab == Tab::Usage => self.cycle_usage_range(),
    _ => {}
},
```

In `loop_.rs`, after handling `Refresh` or when the tab changes to Usage,
issue `GetUsage { range: state.usage.range }` and set `state.usage.rows`.
Use a small helper `fn fetch_usage(...)` to avoid duplication.

- [ ] **Step 7: Cargo check + commit**

```bash
cargo check --workspace
git add crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-core/src/service.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/src/app/loop_.rs \
        crates/llm-relay-tui/src/view/usage.rs \
        crates/llm-relay-tui/src/view/mod.rs \
        crates/llm-relay-tui/tests/usage_state.rs
git commit -m "feat(tui): usage tab with range filter"
```

---

### Task 31: Errors tab

Same pattern as Usage. Errors are recent failed requests / health probe
failures.

**Files:**
- Create: `crates/llm-relay-tui/src/view/errors.rs`
- Modify: `crates/llm-relay-core/src/ipc.rs` (`GetErrors`, `ErrorRow`)
- Modify: `crates/llm-relay-core/src/service.rs` (`get_errors`)
- Modify: `crates/llm-relay-agent/src/ipc_server.rs` (handler)
- Modify: `crates/llm-relay-tui/src/app/state.rs` (`ErrorsState`)
- Modify: `crates/llm-relay-tui/src/view/mod.rs` (route `Tab::Errors`)

- [ ] **Step 1: IPC types**

```rust
// RequestPayload
GetErrors { limit: u32 },

// ResponsePayload
ErrorRows { rows: Vec<ErrorRow> },

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ErrorRow {
    pub timestamp_iso: String,
    pub gateway_name: String,
    pub kind: String,    // "health" | "proxy" | "auth"
    pub message: String,
}
```

- [ ] **Step 2: Service method**

`Service::get_errors(limit)` — query whatever errors table the GUI already
shows (look in `src-tauri/src/lib.rs` for an existing command name like
`list_errors` or similar; reuse the SQL).

- [ ] **Step 3: Errors view**

Create `crates/llm-relay-tui/src/view/errors.rs`:

```rust
use crate::app::state::AppState;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let header = Row::new(vec!["Time", "Gateway", "Kind", "Message"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    let rows = state.errors.rows.iter().map(|r| {
        Row::new(vec![
            Cell::from(r.timestamp_iso.clone()),
            Cell::from(r.gateway_name.clone()),
            Cell::from(r.kind.clone()).style(match r.kind.as_str() {
                "auth" => Style::default().fg(Color::Yellow),
                "proxy" => Style::default().fg(Color::Red),
                _ => Style::default().fg(Color::Magenta),
            }),
            Cell::from(r.message.clone()),
        ])
    });
    let widths = [
        Constraint::Length(20),
        Constraint::Percentage(20),
        Constraint::Length(8),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Recent Errors"))
        .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut ts = TableState::default();
    ts.select(Some(state.errors.selected));
    frame.render_stateful_widget(table, chunks[0], &mut ts);

    let hint = Paragraph::new("r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}
```

- [ ] **Step 4: ErrorsState in state.rs**

```rust
use llm_relay_core::ipc::ErrorRow;
#[derive(Debug, Default)]
pub struct ErrorsState {
    pub rows: Vec<ErrorRow>,
    pub selected: usize,
}
```

Add `pub errors: ErrorsState,` to `AppState`.

- [ ] **Step 5: Wire fetch on activation in loop_**

When `AppEvent::Refresh` fires while `state.active_tab == Tab::Errors`, or
when the user just switched to Errors via Tab/BackTab, send
`GetErrors { limit: 100 }` and store the rows.

- [ ] **Step 6: Cargo check + commit**

```bash
cargo check --workspace
git add crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-core/src/service.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/src/app/loop_.rs \
        crates/llm-relay-tui/src/view/errors.rs \
        crates/llm-relay-tui/src/view/mod.rs
git commit -m "feat(tui): errors tab"
```

---

### Task 32: Settings tab

Read-only summary plus a few toggle actions. Shows: keystore backend in use
(system / encrypted-file), agent PID, IPC socket path, port, log path, and
the auto-launch checkbox the GUI version has.

**Files:**
- Create: `crates/llm-relay-tui/src/view/settings.rs`
- Modify: `crates/llm-relay-core/src/ipc.rs` (`GetSettings`, `Settings`,
  `SetAutoLaunch`)
- Modify: `crates/llm-relay-core/src/service.rs`
- Modify: `crates/llm-relay-agent/src/ipc_server.rs`
- Modify: `crates/llm-relay-tui/src/app/state.rs` (`SettingsState`)
- Modify: `crates/llm-relay-tui/src/view/mod.rs`

- [ ] **Step 1: IPC types**

```rust
// RequestPayload
GetSettings,
SetAutoLaunch { enabled: bool },

// ResponsePayload
SettingsSnapshot { settings: Settings },
SettingsAck,

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub keystore_kind: String,    // "system" | "encrypted-file"
    pub agent_pid: u32,
    pub socket_path: String,
    pub proxy_port: u16,
    pub log_path: String,
    pub auto_launch: bool,
}
```

- [ ] **Step 2: Service methods**

`Service::get_settings()` and `Service::set_auto_launch(bool)`. The
auto-launch behaviour is whatever the existing GUI already does (check
`src-tauri` for an existing command). The keystore_kind is exposed via
the `Keystore::kind()` method added in Phase 5 Task 22.

- [ ] **Step 3: Settings view**

Create `crates/llm-relay-tui/src/view/settings.rs`:

```rust
use crate::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let lines = match state.settings.snapshot.as_ref() {
        None => vec![Line::from("Loading settings...")],
        Some(s) => vec![
            kv("Keystore", &s.keystore_kind, kind_color(&s.keystore_kind)),
            kv("Agent PID", &s.agent_pid.to_string(), Color::White),
            kv("Socket", &s.socket_path, Color::White),
            kv("Proxy port", &s.proxy_port.to_string(), Color::White),
            kv("Log path", &s.log_path, Color::White),
            kv(
                "Auto-launch on boot",
                if s.auto_launch { "ON" } else { "OFF" },
                if s.auto_launch { Color::Green } else { Color::DarkGray },
            ),
        ],
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Settings"));
    frame.render_widget(p, chunks[0]);

    let hint = Paragraph::new("a toggle auto-launch  r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}

fn kv<'a>(k: &'a str, v: &'a str, c: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {k:<20}"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(v.to_string(), Style::default().fg(c)),
    ])
}

fn kind_color(kind: &str) -> Color {
    match kind {
        "system" => Color::Green,
        "encrypted-file" => Color::Yellow,
        _ => Color::Red,
    }
}
```

- [ ] **Step 4: SettingsState**

```rust
use llm_relay_core::ipc::Settings;
#[derive(Debug, Default)]
pub struct SettingsState {
    pub snapshot: Option<Settings>,
}
```

Add `pub settings: SettingsState,` to `AppState`.

- [ ] **Step 5: Wire `a` key in Settings tab**

In `state.rs`, update `Char` arm:

```rust
'a' if self.active_tab == Tab::Settings => {
    // Loop_ will pick this up via a new AppEvent::ToggleAutoLaunch — see step 6.
}
```

Cleaner: add a new variant `AppEvent::ToggleAutoLaunch` and in the
`KeyCode::Char('a')` translation in `loop_.rs`, emit it only when
`state.active_tab == Tab::Settings`. Then in `loop_`'s match, handle
`ToggleAutoLaunch` by sending `SetAutoLaunch { enabled: !current }` and
re-fetching `GetSettings`.

- [ ] **Step 6: Wire fetch on activation in loop_**

Same pattern as Usage and Errors: when activating the Settings tab or on
Refresh, send `GetSettings` and update `state.settings.snapshot`.

- [ ] **Step 7: Cargo check + commit**

```bash
cargo check --workspace
git add crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-core/src/service.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-tui/src/app/event.rs \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/src/app/loop_.rs \
        crates/llm-relay-tui/src/view/settings.rs \
        crates/llm-relay-tui/src/view/mod.rs
git commit -m "feat(tui): settings tab with auto-launch toggle"
```

---


## Phase 10 — Modal dialogs: Add / Edit gateway, Login

Dialogs are full-screen overlays. The renderer detects an active modal in
`AppState` and draws it on top. All key events are routed to the modal first;
unconsumed events fall through to the underlying tab.

### Task 33: Modal infrastructure + Add Gateway dialog

**Files:**
- Create: `crates/llm-relay-tui/src/app/modal.rs`
- Modify: `crates/llm-relay-tui/src/app/state.rs` (`modal: Option<Modal>`)
- Modify: `crates/llm-relay-tui/src/view/mod.rs` (overlay render)
- Create: `crates/llm-relay-tui/src/view/modals/add_gateway.rs`
- Modify: `crates/llm-relay-core/src/ipc.rs` (`AddGateway`)
- Modify: `crates/llm-relay-agent/src/ipc_server.rs`
- Test: `crates/llm-relay-tui/tests/modal_add_gateway.rs`

- [ ] **Step 1: Modal types**

Create `crates/llm-relay-tui/src/app/modal.rs`:

```rust
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
pub enum AddField { Name, Url }
impl Default for AddField { fn default() -> Self { AddField::Name } }

#[derive(Debug, Clone, Default)]
pub struct EditGatewayForm {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub focus: AddField,
    pub error: Option<String>,
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
    WaitingForUser { user_code: String, verification_uri: String, expires_in_secs: u64 },
    Completed,
    Failed(String),
    Expired,
}

/// Routing decision: handled here vs. fall through.
pub enum ModalOutcome {
    Consumed,
    PassThrough,
    Submit(ModalSubmit),
    Close,
}

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
    if f.submitting { return ModalOutcome::Consumed; }
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
            f.focus = match f.focus { AddField::Name => AddField::Url, AddField::Url => AddField::Name };
            ModalOutcome::Consumed
        }
        _ => ModalOutcome::Consumed,
    }
}

fn target_buf(f: &mut AddGatewayForm) -> &mut String {
    match f.focus { AddField::Name => &mut f.name, AddField::Url => &mut f.url }
}

fn edit_handle(_f: &mut EditGatewayForm, _event: &AppEvent) -> ModalOutcome {
    // Same shape as add_handle — implemented in Task 34.
    ModalOutcome::Consumed
}

fn login_handle(event: &AppEvent) -> ModalOutcome {
    match event {
        AppEvent::Esc => ModalOutcome::Close,
        // Implemented in Task 35.
        _ => ModalOutcome::Consumed,
    }
}
```

- [ ] **Step 2: Wire Modal into state**

In `crates/llm-relay-tui/src/app/state.rs`:

```rust
pub mod modal_re_export {
    pub use crate::app::modal::*;
}

// In AppState struct:
pub modal: Option<crate::app::modal::Modal>,
```

Update `pub mod modal;` in `app/mod.rs`.

In `AppState::handle`, before the existing match, intercept when a modal is
open — but routing decisions live in `loop_` (since they may need to issue
async IPC). Keep `state.modal` access pure and let `loop_` orchestrate.

- [ ] **Step 3: Add `AddGateway` IPC**

In `crates/llm-relay-core/src/ipc.rs`:

```rust
// RequestPayload
AddGateway { name: String, url: String },

// ResponsePayload
GatewayCreated { id: uuid::Uuid },
```

In `ipc_server.rs`:

```rust
RequestPayload::AddGateway { name, url } => {
    match service.add_gateway(name, url).await {
        Ok(id) => ResponsePayload::GatewayCreated { id },
        Err(e) => ResponsePayload::Error { message: e.to_string() },
    }
}
```

`Service::add_gateway` already exists from Phase 4 — confirm signature.

- [ ] **Step 4: Implement Add Gateway view**

Create `crates/llm-relay-tui/src/view/modals/mod.rs`:
```rust
pub mod add_gateway;
```
Create `crates/llm-relay-tui/src/view/modals/add_gateway.rs`:

```rust
use crate::app::modal::{AddField, AddGatewayForm};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, form: &AddGatewayForm) {
    let dialog = centered(60, 11, area);
    frame.render_widget(Clear, dialog);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add Gateway ")
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), dialog);

    let inner = block.inner(dialog);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(field_label("Name", form.focus == AddField::Name), chunks[0]);
    frame.render_widget(field_value(&form.name, form.focus == AddField::Name), chunks[1]);
    frame.render_widget(field_label("URL", form.focus == AddField::Url), chunks[2]);
    frame.render_widget(field_value(&form.url, form.focus == AddField::Url), chunks[3]);

    if let Some(err) = &form.error {
        let p = Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red));
        frame.render_widget(p, chunks[4]);
    } else if form.submitting {
        let p = Paragraph::new("Submitting...").style(Style::default().fg(Color::Yellow));
        frame.render_widget(p, chunks[4]);
    }

    let hint = Paragraph::new("↑/↓ field  Enter submit  Esc cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[5]);
}

fn field_label(label: &str, focused: bool) -> Paragraph<'_> {
    let style = if focused {
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    Paragraph::new(label).style(style)
}

fn field_value<'a>(value: &'a str, focused: bool) -> Paragraph<'a> {
    let display = if focused { format!("> {value}_") } else { format!("  {value}") };
    Paragraph::new(display).block(Block::default().borders(Borders::BOTTOM))
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
```

In `view/mod.rs`, after the per-tab `match`, add:

```rust
if let Some(modal) = &state.modal {
    use crate::app::modal::Modal;
    match modal {
        Modal::AddGateway(f) => modals::add_gateway::render(frame, frame.size(), f),
        Modal::EditGateway(_f) => { /* Task 34 */ }
        Modal::Login(_f) => { /* Task 35 */ }
    }
}
```

Add `pub mod modals;` near top of `view/mod.rs`.

- [ ] **Step 5: Open dialog on `a` in Gateways tab**

Update `loop_`'s key translation (or `state.handle`):

```rust
'a' if self.active_tab == Tab::Gateways && self.modal.is_none() => {
    self.modal = Some(crate::app::modal::Modal::AddGateway(Default::default()));
}
```

In `loop_`'s event branch, **before** delegating to `state.handle`, check
if a modal is open and route through `Modal::handle`:

```rust
if let Some(modal) = state.modal.as_mut() {
    use crate::app::modal::ModalOutcome;
    match modal.handle(&evt) {
        ModalOutcome::Consumed => {}
        ModalOutcome::Close => state.modal = None,
        ModalOutcome::PassThrough => state.handle(evt),
        ModalOutcome::Submit(ms) => {
            handle_submit(&client, &mut state, ms).await;
        }
    }
    term.draw(|f| crate::view::render(f, &state))?;
    if state.should_quit { break; }
    continue;
}
```

`handle_submit` for `AddGateway`:
```rust
async fn handle_submit(
    client: &Arc<IpcClient>,
    state: &mut AppState,
    submit: ModalSubmit,
) {
    use llm_relay_core::ipc::*;
    if let ModalSubmit::AddGateway { name, url } = submit {
        if let Some(Modal::AddGateway(f)) = state.modal.as_mut() {
            f.submitting = true; f.error = None;
        }
        match client.request(RequestPayload::AddGateway { name, url }).await {
            Ok(ResponsePayload::GatewayCreated { .. }) => {
                state.modal = None;
                if let Ok(ResponsePayload::GatewayList { gateways }) =
                    client.request(RequestPayload::ListGateways).await {
                    state.replace_gateways(gateways.into_iter().map(into_row).collect());
                }
            }
            Ok(_) => { /* unexpected */ }
            Err(e) => {
                if let Some(Modal::AddGateway(f)) = state.modal.as_mut() {
                    f.submitting = false;
                    f.error = Some(e.to_string());
                }
            }
        }
    }
    // EditGateway handled symmetrically — see Task 34.
}
```

- [ ] **Step 6: Test (state-only)**

In `crates/llm-relay-tui/tests/modal_add_gateway.rs`:

```rust
use llm_relay_tui::app::modal::{AddGatewayForm, Modal, ModalOutcome, ModalSubmit};
use llm_relay_tui::app::event::AppEvent;

#[test]
fn enter_with_blank_name_sets_error_and_does_not_submit() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    let outcome = m.handle(&AppEvent::Enter);
    assert!(matches!(outcome, ModalOutcome::Consumed));
    if let Modal::AddGateway(f) = &m {
        assert!(f.error.is_some());
    }
}

#[test]
fn typing_into_name_then_url_then_enter_submits_with_values() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    for c in "gw1".chars() { m.handle(&AppEvent::Char(c)); }
    m.handle(&AppEvent::Down); // focus URL
    for c in "https://x".chars() { m.handle(&AppEvent::Char(c)); }
    let outcome = m.handle(&AppEvent::Enter);
    match outcome {
        ModalOutcome::Submit(ModalSubmit::AddGateway { name, url }) => {
            assert_eq!(name, "gw1");
            assert_eq!(url, "https://x");
        }
        other => panic!("expected submit, got {other:?}"),
    }
}

#[test]
fn esc_closes() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}
```

(Add `#[derive(Debug)]` to `ModalOutcome` for the panic format.)

Run: `cargo test -p llm-relay-tui --test modal_add_gateway` → PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/llm-relay-tui/src/app/modal.rs \
        crates/llm-relay-tui/src/app/mod.rs \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/src/app/loop_.rs \
        crates/llm-relay-tui/src/view/modals/ \
        crates/llm-relay-tui/src/view/mod.rs \
        crates/llm-relay-core/src/ipc.rs \
        crates/llm-relay-agent/src/ipc_server.rs \
        crates/llm-relay-tui/tests/modal_add_gateway.rs
git commit -m "feat(tui): add gateway modal dialog"
```

---

### Task 34: Edit Gateway dialog

Almost identical to Add. Open with `e`, prefilled from selected row,
submits to `UpdateGateway` IPC.

- [ ] **Step 1: Add `UpdateGateway` IPC**

```rust
// RequestPayload
UpdateGateway { id: uuid::Uuid, name: String, url: String },
// ResponsePayload
GatewayUpdated { id: uuid::Uuid },
```

Wire `Service::update_gateway` and the agent handler exactly mirroring
`AddGateway` from Task 33.

- [ ] **Step 2: Implement `edit_handle` and `EditGateway` view**

Replace the stub `edit_handle` in `app/modal.rs`:

```rust
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
                id: f.id, name: f.name.clone(), url: f.url.clone(),
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
            f.focus = if f.focus == AddField::Name { AddField::Url } else { AddField::Name };
            ModalOutcome::Consumed
        }
        _ => ModalOutcome::Consumed,
    }
}
```

Create `crates/llm-relay-tui/src/view/modals/edit_gateway.rs` — copy
add_gateway.rs and change the title from "Add Gateway" to "Edit Gateway".
Wire `pub mod edit_gateway;` in `view/modals/mod.rs` and route in
`view/mod.rs`.

- [ ] **Step 3: Open Edit on `e` from Gateways tab**

In `loop_` (or wherever you placed `'a' => open AddGateway`):

```rust
'e' if state.active_tab == Tab::Gateways && state.modal.is_none() => {
    if let Some(row) = state.gateways.get(state.selected_row) {
        state.modal = Some(Modal::EditGateway(EditGatewayForm {
            id: row.id, name: row.name.clone(), url: row.url.clone(),
            focus: AddField::Name, error: None,
        }));
    }
}
```

- [ ] **Step 4: Extend `handle_submit`**

```rust
ModalSubmit::EditGateway { id, name, url } => {
    match client.request(RequestPayload::UpdateGateway { id, name, url }).await {
        Ok(ResponsePayload::GatewayUpdated { .. }) => {
            state.modal = None;
            // Refresh list
            if let Ok(ResponsePayload::GatewayList { gateways }) =
                client.request(RequestPayload::ListGateways).await {
                state.replace_gateways(gateways.into_iter().map(into_row).collect());
            }
        }
        Ok(_) => {}
        Err(e) => {
            if let Some(Modal::EditGateway(f)) = state.modal.as_mut() {
                f.error = Some(e.to_string());
            }
        }
    }
}
```

- [ ] **Step 5: Cargo check + commit**

```bash
cargo check --workspace
git add ...
git commit -m "feat(tui): edit gateway modal dialog"
```

---

### Task 35: Login dialog (URL + code box, copy)

Opened with `l` on Gateways tab. Sends `StartLogin`, displays `user_code`
and `verification_uri`, listens for `LoginCompleted | LoginFailed |
LoginExpired` events on the IPC bus, and updates `state.modal` accordingly.

**Files:**
- Create: `crates/llm-relay-tui/src/view/modals/login.rs`
- Modify: `crates/llm-relay-tui/src/app/modal.rs` (login_handle full impl)
- Modify: `crates/llm-relay-tui/src/app/loop_.rs` (start login + event routing)

- [ ] **Step 1: Implement `login_handle`**

```rust
fn login_handle(event: &AppEvent) -> ModalOutcome {
    match event {
        AppEvent::Esc => ModalOutcome::Close, // also sends CancelLogin in loop_
        AppEvent::Char('c') => ModalOutcome::Consumed, // copy handled in loop_ — needs IO
        _ => ModalOutcome::Consumed,
    }
}
```

- [ ] **Step 2: Login view**

Create `crates/llm-relay-tui/src/view/modals/login.rs`:

```rust
use crate::app::modal::{LoginForm, LoginUiState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, form: &LoginForm) {
    let dialog = centered(70, 13, area);
    frame.render_widget(Clear, dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Sign in to {} ", form.gateway_name))
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), dialog);
    let inner = block.inner(dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let (body, hint) = match &form.state {
        LoginUiState::Initiating => (
            vec![Line::from("Requesting device code...")],
            "Esc cancel",
        ),
        LoginUiState::WaitingForUser { user_code, verification_uri, expires_in_secs } => {
            let lines = vec![
                Line::from(vec![
                    Span::styled("Open this URL in any browser:", Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(Span::styled(verification_uri.clone(), Style::default().fg(Color::Cyan))),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Enter the code:", Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(Span::styled(
                    user_code.clone(),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("Expires in {expires_in_secs}s")),
            ];
            (lines, "c copy code  Esc cancel")
        }
        LoginUiState::Completed => (
            vec![Line::from(Span::styled("✔ Signed in successfully", Style::default().fg(Color::Green)))],
            "Esc close",
        ),
        LoginUiState::Failed(msg) => (
            vec![Line::from(Span::styled(format!("✘ {msg}"), Style::default().fg(Color::Red)))],
            "Esc close",
        ),
        LoginUiState::Expired => (
            vec![Line::from(Span::styled("⏱ Code expired — please try again", Style::default().fg(Color::Yellow)))],
            "Esc close",
        ),
    };

    let p = Paragraph::new(body);
    frame.render_widget(p, chunks[1]);
    let h = Paragraph::new(hint).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(h, chunks[2]);
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
```

Wire in `view/modals/mod.rs` and `view/mod.rs`.

- [ ] **Step 3: Open + close lifecycle in loop_**

When `'l'` is pressed on Gateways tab with no active modal:

```rust
'l' if state.active_tab == Tab::Gateways && state.modal.is_none() => {
    if let Some(row) = state.gateways.get(state.selected_row) {
        let gid = row.id;
        let gname = row.name.clone();
        state.modal = Some(Modal::Login(LoginForm {
            gateway_id: gid,
            gateway_name: gname,
            state: LoginUiState::Initiating,
        }));
        // Fire StartLogin asynchronously, then patch state with the response.
        match client.request(RequestPayload::StartLogin { gateway_id: gid }).await {
            Ok(ResponsePayload::LoginInitiated { user_code, verification_uri, expires_in_secs, .. }) => {
                if let Some(Modal::Login(f)) = state.modal.as_mut() {
                    f.state = LoginUiState::WaitingForUser { user_code, verification_uri, expires_in_secs };
                }
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(Modal::Login(f)) = state.modal.as_mut() {
                    f.state = LoginUiState::Failed(e.to_string());
                }
            }
        }
    }
}
```

When the user closes a login dialog with `Esc` while it's `Initiating` or
`WaitingForUser`, send `CancelLogin`:

```rust
// After ModalOutcome::Close arm, before clearing state.modal:
if let Some(Modal::Login(f)) = &state.modal {
    if matches!(f.state, LoginUiState::Initiating | LoginUiState::WaitingForUser { .. }) {
        let _ = client.request(RequestPayload::CancelLogin { gateway_id: f.gateway_id }).await;
    }
}
state.modal = None;
```

When an `Ipc(Event::LoginCompleted | LoginFailed | LoginExpired)` event
arrives, patch the modal:

In `state.apply_ipc`:

```rust
IpcEvent::LoginCompleted { gateway_id, .. } => {
    if let Some(Modal::Login(f)) = self.modal.as_mut() {
        if f.gateway_id == gateway_id {
            f.state = LoginUiState::Completed;
        }
    }
    // The agent has already persisted the session token; refresh request
    // is fired from loop_ on the next tick.
}
IpcEvent::LoginFailed { gateway_id, message } => {
    if let Some(Modal::Login(f)) = self.modal.as_mut() {
        if f.gateway_id == gateway_id {
            f.state = LoginUiState::Failed(message);
        }
    }
}
IpcEvent::LoginExpired { gateway_id } => {
    if let Some(Modal::Login(f)) = self.modal.as_mut() {
        if f.gateway_id == gateway_id {
            f.state = LoginUiState::Expired;
        }
    }
}
```

- [ ] **Step 4: Implement `c` → copy code to clipboard**

In `loop_`'s key handling, when a Login modal is open and key is `'c'`:

```rust
'c' => {
    if let Some(Modal::Login(LoginForm { state: LoginUiState::WaitingForUser { user_code, .. }, .. })) = &state.modal {
        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(user_code.clone()));
        state.status_message = Some("Code copied".into());
    }
}
```

Add `arboard = "3"` to `crates/llm-relay-tui/Cargo.toml` dependencies.
On Linux servers without X/Wayland, `arboard` will fail silently — that's
acceptable; the URL+code are still visible.

- [ ] **Step 5: Cargo check + commit**

```bash
cargo check --workspace
git add crates/llm-relay-tui/src/view/modals/login.rs \
        crates/llm-relay-tui/src/view/modals/mod.rs \
        crates/llm-relay-tui/src/view/mod.rs \
        crates/llm-relay-tui/src/app/modal.rs \
        crates/llm-relay-tui/src/app/state.rs \
        crates/llm-relay-tui/src/app/loop_.rs \
        crates/llm-relay-tui/Cargo.toml
git commit -m "feat(tui): device-code login dialog"
```

---


## Phase 11 — Lifecycle edge cases & cross-platform polish

The first 10 phases handled happy path. This phase locks down the failure
modes the review flagged as Q2: stale socket, PID reuse, lock race, kill -9
recovery. We add integration tests that exercise the lifecycle guard end to
end against a real binary.

### Task 36: Lifecycle integration test harness

**Files:**
- Create: `crates/llm-relay-agent/tests/lifecycle_integration.rs`
- Create: `crates/llm-relay-agent/tests/support/mod.rs`

- [ ] **Step 1: Test support helpers**

In `crates/llm-relay-agent/tests/support/mod.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct AgentBin {
    pub bin: PathBuf,
}

impl AgentBin {
    pub fn locate() -> Self {
        // CARGO_BIN_EXE_<name> is set by cargo when building the test crate
        // for binaries declared in the same package. The agent binary is in
        // its own package, so we walk back from the test exe instead.
        let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()    // crates/
            .parent().unwrap()    // workspace root
            .join("target")
            .join(if cfg!(debug_assertions) { "debug" } else { "release" })
            .join(if cfg!(windows) { "llm-relay-agent.exe" } else { "llm-relay-agent" });
        assert!(bin.exists(), "agent binary not built — run `cargo build -p llm-relay-agent` first");
        Self { bin }
    }

    pub fn spawn(&self, runtime_dir: &Path) -> Child {
        Command::new(&self.bin)
            .env("LLM_RELAY_RUNTIME_DIR", runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agent")
    }
}

pub fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

pub fn wait_for_no_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
```

The agent must respect `LLM_RELAY_RUNTIME_DIR` env override. Update the
`paths` module from Phase 5 Task 18 to honor it:

```rust
pub fn runtime_dir() -> std::io::Result<std::path::PathBuf> {
    if let Ok(p) = std::env::var("LLM_RELAY_RUNTIME_DIR") {
        return Ok(p.into());
    }
    // existing platform default lookup
    // ...
}
```

- [ ] **Step 2: Stale socket recovery test**

In `crates/llm-relay-agent/tests/lifecycle_integration.rs`:

```rust
#[path = "support/mod.rs"]
mod support;
use support::*;
use std::time::Duration;

#[cfg_attr(windows, ignore = "uses Unix socket semantics")]
#[test]
fn agent_recovers_from_stale_socket_left_by_killed_process() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();

    // Spawn agent, wait for socket, kill -9, restart, expect success.
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(wait_for_socket(&sock, Duration::from_secs(5)), "agent did not bind socket");

    // SIGKILL — leaves socket file dangling.
    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGKILL); }
    let _ = child.wait();
    // Socket file persists after SIGKILL on unix — confirm.
    assert!(sock.exists(), "stale socket should still be on disk");

    // Restart should succeed by removing the stale socket.
    let mut child2 = bin.spawn(dir.path());
    assert!(wait_for_socket(&sock, Duration::from_secs(5)), "restart did not rebind");

    // Cleanup
    #[cfg(unix)]
    unsafe { libc::kill(child2.id() as i32, libc::SIGTERM); }
    let _ = child2.wait();
}
```

- [ ] **Step 3: PID-reuse / lock race test**

```rust
#[test]
fn second_agent_refuses_to_start_when_first_holds_lock() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();

    let mut a = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(wait_for_socket(&sock, std::time::Duration::from_secs(5)));

    // Second invocation must exit non-zero (lock held).
    let mut b = bin.spawn(dir.path());
    let status = b.wait_with_output().expect("collect output");
    assert!(!status.status.success(), "second agent should refuse to start");
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        stderr.contains("already running") || stderr.contains("lock"),
        "expected lock error, got: {stderr}"
    );

    #[cfg(unix)]
    unsafe { libc::kill(a.id() as i32, libc::SIGTERM); }
    let _ = a.wait();
}
```

- [ ] **Step 4: PID reuse in pidfile**

```rust
#[test]
fn agent_starts_when_pidfile_holds_a_pid_no_longer_alive() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-write a pidfile pointing at a guaranteed-dead PID. PID 0 is
    // never a real process; on Unix `kill(0, 0)` would refer to the
    // process group, so use u32::MAX which is reliably non-existent.
    std::fs::write(dir.path().join("agent.pid"), "4294967295").unwrap();
    // Also drop a stale socket file in.
    std::fs::write(dir.path().join("agent.sock"), b"").unwrap();

    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(
        wait_for_socket(&sock, std::time::Duration::from_secs(5)),
        "agent should start despite stale pidfile + socket"
    );

    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
    let _ = child.wait();
}
```

Note: `agent.sock` after replacement will be a Unix socket inode, so the
pre-written file gets removed by the agent and replaced. Test only
verifies the agent doesn't error out.

- [ ] **Step 5: Graceful shutdown removes pidfile + socket**

```rust
#[test]
fn graceful_shutdown_removes_pidfile_and_socket() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());

    let sock = dir.path().join("agent.sock");
    let pidf = dir.path().join("agent.pid");
    assert!(wait_for_socket(&sock, std::time::Duration::from_secs(5)));
    assert!(pidf.exists());

    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
    let _ = child.wait();

    assert!(wait_for_no_socket(&sock, std::time::Duration::from_secs(3)),
        "socket should be cleaned up on graceful exit");
    assert!(!pidf.exists(), "pidfile should be cleaned up on graceful exit");
}
```

- [ ] **Step 6: Run all lifecycle tests**

```bash
cargo build -p llm-relay-agent
cargo test -p llm-relay-agent --test lifecycle_integration -- --test-threads=1
```

Expected: PASS (4 tests). `--test-threads=1` because they all share the
binary location and may compete for OS resources.

- [ ] **Step 7: Commit**

```bash
git add crates/llm-relay-agent/tests/lifecycle_integration.rs \
        crates/llm-relay-agent/tests/support/mod.rs \
        crates/llm-relay-core/src/paths.rs
git commit -m "test(agent): lifecycle edge cases — stale socket, lock race, PID reuse"
```

---

### Task 37: GUI vs TUI mutual exclusion

The GUI keeps its embedded proxy; the TUI runs the agent. The conflict
surface is the proxy port (18080) and the lock file. Both binaries must
detect contention and refuse cleanly.

**Files:**
- Modify: `src-tauri/src/lib.rs` (port-bind check before starting embedded proxy)
- Modify: `crates/llm-relay-agent/src/main.rs` (already has lock check from
  Phase 5, just confirm port also probed)
- Test: `crates/llm-relay-agent/tests/mutual_exclusion.rs`

- [ ] **Step 1: Have GUI startup probe the port and lock**

In `src-tauri/src/lib.rs`, before binding the embedded proxy, attempt a
non-blocking `TcpListener::bind("127.0.0.1:18080")`. On `AddrInUse`, show
a Tauri dialog explaining "An LLM Relay agent appears to be running. Stop
it first or close this window." and exit. Pseudocode:

```rust
match std::net::TcpListener::bind(("127.0.0.1", 18080)) {
    Ok(listener) => { drop(listener); /* proceed with proxy startup */ }
    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
        // Show error dialog, exit cleanly.
        tauri_plugin_dialog::DialogExt::dialog(&app)
            .message("Port 18080 is already in use by another LLM Relay process.\n\nStop the TUI agent or other instance, then relaunch.")
            .blocking_show();
        std::process::exit(1);
    }
    Err(e) => return Err(e.into()),
}
```

Also probe the lock file (same path the agent uses) and refuse if held —
this catches the case where someone bound :18080 differently but our agent
still owns its lock.

- [ ] **Step 2: Confirm agent main also exits cleanly when port is taken**

Phase 5 Task 19 already wires the lock guard. Add a port check at the same
point: if `TcpListener::bind("127.0.0.1:18080")` returns `AddrInUse`, log
"port 18080 already in use — is the GUI app running?" to stderr and exit
with code 2.

- [ ] **Step 3: Mutual-exclusion test**

In `crates/llm-relay-agent/tests/mutual_exclusion.rs`:

```rust
#[path = "support/mod.rs"]
mod support;
use support::*;

#[test]
fn agent_refuses_when_port_18080_already_bound() {
    // Bind 127.0.0.1:18080 from the test process to simulate the GUI.
    let _hold = std::net::TcpListener::bind(("127.0.0.1", 18080))
        .expect("bind 18080 — make sure no llm-relay is running");

    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();
    let child = bin.spawn(dir.path());
    let out = child.wait_with_output().expect("collect");
    assert!(!out.status.success(), "agent should refuse with port in use");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("port 18080") || stderr.contains("AddrInUse"),
        "expected port-in-use error, got: {stderr}");
}
```

- [ ] **Step 4: Run + commit**

```bash
cargo test -p llm-relay-agent --test mutual_exclusion -- --test-threads=1
git add src-tauri/src/lib.rs \
        crates/llm-relay-agent/src/main.rs \
        crates/llm-relay-agent/tests/mutual_exclusion.rs
git commit -m "feat: mutual exclusion between GUI and TUI agent on port 18080"
```

---

### Task 38: Reconnect on agent crash

If the agent dies while the TUI is open, the IPC client's reader loop sees
EOF and clears pending requests. The TUI should:
1. Show a status banner.
2. Attempt to reconnect (or attach-or-spawn) every 2 seconds.
3. When reconnected, refetch state.

**Files:**
- Modify: `crates/llm-relay-tui/src/ipc_client.rs` (expose a "disconnected"
  signal — broadcast a synthetic `Event::AgentDisconnected` or close a
  watch channel)
- Modify: `crates/llm-relay-tui/src/app/loop_.rs`

- [ ] **Step 1: Add a disconnect signal**

In `ipc_client.rs`, add:

```rust
events_tx: broadcast::Sender<Event>,
disconnected_tx: tokio::sync::watch::Sender<bool>,

pub fn disconnected(&self) -> tokio::sync::watch::Receiver<bool> {
    self.disconnected_tx.subscribe()
}
```

Initialize with `let (disc_tx, _) = watch::channel(false);` and after the
reader loop exits, set it: `let _ = disconnected_tx.send(true);`.

- [ ] **Step 2: Reconnect logic in loop_**

Watch `client.disconnected()` in a separate select-arm. When it goes true,
set `state.status_message = Some("Agent disconnected — reconnecting...")`,
then call `bootstrap::ensure_agent` in a retry loop until it returns a
new client. Replace `client` (move into an `Arc<Mutex<Arc<IpcClient>>>` so
the reader and writer paths stay coherent), redraw, refetch.

Sketch:

```rust
let mut disc = client.disconnected();
loop {
    tokio::select! {
        _ = disc.changed() => {
            if *disc.borrow() {
                state.status_message = Some("Agent disconnected — reconnecting...".into());
                term.draw(|f| crate::view::render(f, &state))?;
                loop {
                    match bootstrap::ensure_agent(&socket, EnsureMode::AttachOrSpawn).await {
                        Ok(AgentHandle::Attached(c)) | Ok(AgentHandle::Spawned { client: c, .. }) => {
                            *client_slot.lock().await = c.clone();
                            disc = c.disconnected();
                            // refetch
                            break;
                        }
                        Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                    }
                }
                state.status_message = None;
            }
        }
        Some(evt) = rx.recv() => {
            // existing event handling
        }
    }
}
```

This is intrusive — the whole loop needs to be rewritten around `select!`.
Take a clean pass and verify every `client.request(...)` site goes through
`client_slot.lock().await.clone()`.

- [ ] **Step 3: Smoke test**

`cargo build -p llm-relay-agent -p llm-relay-tui` then run the TUI in one
terminal, kill the agent (`pkill -9 llm-relay-agent`), confirm:
- Status banner appears
- Banner clears once agent restarts (or if you don't restart it, TUI keeps
  trying every 2s)
- After reconnect, gateway list re-renders with current data

This is a manual smoke step — no automated test added.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-tui/src/ipc_client.rs \
        crates/llm-relay-tui/src/app/loop_.rs
git commit -m "feat(tui): auto-reconnect on agent disconnect"
```

---


## Phase 12 — systemd unit, README, CI matrix

The final phase ships operations docs, a sample systemd unit for headless
Linux deployment, and a CI matrix that exercises the cross-platform code
paths on macOS / Linux / Windows.

### Task 39: systemd unit + README

**Files:**
- Create: `packaging/systemd/llm-relay-agent.service`
- Create: `packaging/systemd/README.md`
- Modify: `README.md` (top-level — add TUI section + headless server quickstart)

- [ ] **Step 1: Write systemd unit**

Create `packaging/systemd/llm-relay-agent.service`:

```ini
[Unit]
Description=LLM Relay agent (headless gateway proxy)
Documentation=https://github.com/<org>/llm-relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# The agent does its own daemonization when run interactively, but under
# systemd we want it to stay foreground so journald captures stdout/stderr.
Environment=LLM_RELAY_FOREGROUND=1
Environment=LLM_RELAY_RUNTIME_DIR=%h/.local/state/llm-relay
ExecStart=/usr/local/bin/llm-relay-agent
Restart=on-failure
RestartSec=5s

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.local/state/llm-relay %h/.local/share/llm-relay
PrivateTmp=true

[Install]
WantedBy=default.target
```

Note: `LLM_RELAY_FOREGROUND=1` is a new env knob the agent must honor to
skip its `daemonize` call. Wire this into `agent/src/main.rs` (Phase 5
Task 21) as a one-line conditional.

- [ ] **Step 2: Write packaging README**

Create `packaging/systemd/README.md`:

```markdown
# Running the LLM Relay agent under systemd

For headless Linux servers (no GUI), run the agent as a per-user systemd
service.

## Install

1. Build the agent and install the binary:
   ```sh
   cargo build --release -p llm-relay-agent
   sudo install -m 0755 target/release/llm-relay-agent /usr/local/bin/
   ```

2. Drop the unit into your user systemd dir:
   ```sh
   mkdir -p ~/.config/systemd/user
   cp packaging/systemd/llm-relay-agent.service ~/.config/systemd/user/
   ```

3. Enable + start:
   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now llm-relay-agent.service
   ```

4. Linger so the agent runs without an active session:
   ```sh
   sudo loginctl enable-linger "$USER"
   ```

## Verify

```sh
systemctl --user status llm-relay-agent
journalctl --user -u llm-relay-agent -f
```

Then attach with the TUI:
```sh
llm-relay-tui
```

## Keystore

On a server without DBus / GNOME-Keyring / KWallet, the agent automatically
falls back to an encrypted file at `~/.local/state/llm-relay/secrets.enc`.
The encryption key is derived (via Argon2) from a passphrase you set on
first launch (`llm-relay-tui` will prompt). To change the passphrase:

```sh
llm-relay-tui --change-passphrase
```
```

- [ ] **Step 3: Add a TUI section to top-level README.md**

Append to `README.md`:

```markdown
## Terminal UI

For headless servers or when you prefer the terminal:

```sh
cargo build --release -p llm-relay-tui -p llm-relay-agent
./target/release/llm-relay-tui
```

The TUI auto-spawns a detached agent on first launch. To run the agent as
a persistent systemd service, see [packaging/systemd/](packaging/systemd/).

Keys:
- `Tab` / `Shift+Tab` — switch tabs
- `↑` / `↓` — select row, `Enter` — expand
- `a` add gateway, `e` edit, `l` login, `s` star, `r` refresh
- `q` quit (agent keeps running)
```

- [ ] **Step 4: Commit**

```bash
git add packaging/systemd/ README.md \
        crates/llm-relay-agent/src/main.rs
git commit -m "docs: systemd unit, headless quickstart, TUI README section"
```

---

### Task 40: CI matrix smoke

**Files:**
- Create: `.github/workflows/tui-ci.yml` (or modify existing CI)

- [ ] **Step 1: Add a workflow that builds + runs unit tests on the matrix**

Create `.github/workflows/tui-ci.yml`:

```yaml
name: TUI CI

on:
  push:
    branches: [main]
    paths:
      - "crates/**"
      - "src-tauri/**"
      - ".github/workflows/tui-ci.yml"
  pull_request:
    paths:
      - "crates/**"
      - "src-tauri/**"

jobs:
  test:
    name: ${{ matrix.os }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Build agent + TUI
        run: cargo build -p llm-relay-agent -p llm-relay-tui --verbose

      - name: Unit tests (core)
        run: cargo test -p llm-relay-core --verbose

      - name: Unit tests (agent)
        run: cargo test -p llm-relay-agent --lib --verbose

      - name: Unit tests (tui state)
        run: |
          cargo test -p llm-relay-tui --test app_state
          cargo test -p llm-relay-tui --test gateways_view
          cargo test -p llm-relay-tui --test usage_state
          cargo test -p llm-relay-tui --test modal_add_gateway
          cargo test -p llm-relay-tui --test ipc_client_roundtrip
          cargo test -p llm-relay-tui --test bootstrap

      - name: Lifecycle integration tests
        # Lifecycle tests bind ports + spawn child processes; serialize them.
        run: cargo test -p llm-relay-agent --test lifecycle_integration -- --test-threads=1

      - name: Mutual exclusion (agent vs another process holding 18080)
        if: matrix.os != 'windows-latest'
        run: cargo test -p llm-relay-agent --test mutual_exclusion -- --test-threads=1

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Verify locally that everything in the matrix passes**

```bash
cargo build -p llm-relay-agent -p llm-relay-tui
cargo test -p llm-relay-core
cargo test -p llm-relay-agent
cargo test -p llm-relay-tui
cargo clippy --workspace --all-targets -- -D warnings
```

Fix any clippy lints inline before committing the workflow.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/tui-ci.yml
git commit -m "ci: build + test TUI/agent on macOS, Linux, Windows"
```

---

## Plan Self-Review

Before merging the phase files into the main plan document, walk through
this checklist:

### Spec coverage

For each section of `docs/superpowers/specs/2026-04-20-tui-design.md`,
identify the implementing task(s):

- Architecture (workspace + crates) → Phase 1, Tasks 1-7
- Process model & lifecycle → Phase 5 Tasks 18-19, Phase 7 Task 24, Phase 11 Tasks 36-37
- IPC protocol (length-prefix + request_id + Event channel) → Phase 3 Tasks 11-13, Phase 7 Task 25
- TUI layout (4 tabs) → Phase 8 Tasks 27-29, Phase 9 Tasks 30-32
- Modal dialogs → Phase 10 Tasks 33-35
- Keychain (system + encrypted-file fallback w/ probe) → Phase 1 Task 4, Phase 2 Tasks 8-10
- Login (device-code, server interval, key_id) → Phase 6 Task 23, Phase 10 Task 35
- Error handling & reconnect → Phase 11 Task 38
- Tests (stale sock, PID reuse, lock race, kill -9) → Phase 11 Task 36
- YAGNI items → none implemented (correct)
- systemd + README + CI → Phase 12 Tasks 39-40

If any spec section is missing, add a task before merging.

### Placeholder scan

Re-read every phase file and grep for these strings:
- `TBD`, `TODO`, `FIXME`
- "fill in", "implement later", "appropriate", "as needed"
- bare `// ...` ellipses inside code blocks (allowed only when explicitly
  pointing at something defined in another step)

### Type consistency

Cross-check that names and signatures match across phases:
- `Service` method names referenced in agent handlers (Phases 4-9-10)
- `RequestPayload` / `ResponsePayload` / `Event` variant names match
  between `core::ipc` (Phase 3) and the TUI/agent users (Phases 6-10)
- `IpcClient::request` signature matches caller usage everywhere
- `LoginRegistry::start` signature matches the IPC handler in Phase 6
- `GatewayRow` / `GatewaySummary` field names match between `state` and
  `ipc` modules

### Merge

After self-review, append phase files to the main plan:

```bash
cd docs/superpowers/plans
for n in 2 3 4 5 6 7 8 9 10 11 12; do
  printf '\n\n' >> 2026-04-20-tui-implementation.md
  cat 2026-04-20-tui-implementation.md.phase$n >> 2026-04-20-tui-implementation.md
done
rm 2026-04-20-tui-implementation.md.phase*
git add 2026-04-20-tui-implementation.md
git commit -m "docs: assemble full TUI implementation plan"
```

---
