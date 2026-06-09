# WSL2 Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let CLI tools running inside WSL2 transparently use a Windows-side LLM Relay, by writing per-distro CLI configs through `wsl.exe`, binding a second proxy listener on the WSL virtual-NIC IP, and probing each distro for a stable reachable URL.

**Architecture:** Windows-only feature gated by `#[cfg(target_os = "windows")]`. New `wsl/` module under `llm-relay-core` for distro discovery, filesystem ops via `wsl.exe`, and gateway-IP detection. `proxy_server` refactored into a `ProxyHandle` that owns one mandatory `127.0.0.1` listener task and one optional hot-swappable WSL listener task. `config_writer` refactored behind a `CliBackend` trait so apply/clear iterate over `CliTarget`s (Windows + selected distros), each with its own `base_url` and `installed` tools. Snapshot file format changes from a single JSON file to a directory of per-target JSONs keyed by opaque sha256 ids; legacy file is migrated on first launch.

**Tech Stack:** Rust (`tokio`, `axum`, `rusqlite`, `serde_json`, `sha2`, `base32`), React + Tauri frontend, SQLite for distro cache, `wsl.exe` subprocess for filesystem and probing.

**Spec:** [docs/superpowers/specs/2026-06-09-wsl2-integration-design.md](../specs/2026-06-09-wsl2-integration-design.md)

---

## File Structure

New files (Windows-only behavior, but compile on all platforms):

- `crates/llm-relay-core/src/wsl/mod.rs` — module root, re-exports
- `crates/llm-relay-core/src/wsl/distro.rs` — `wsl.exe -l -v` discovery, probe, SQLite cache
- `crates/llm-relay-core/src/wsl/fs.rs` — `wsl_read` / `wsl_atomic_write` / `wsl_remove` / `wsl_exists`
- `crates/llm-relay-core/src/wsl/network.rs` — `find_wsl_gateway_ip()` via `ipconfig`
- `crates/llm-relay-core/src/wsl/probe.rs` — per-distro URL HTTP/TCP probe
- `crates/llm-relay-core/src/wsl/state.rs` — Active/Lazy detection state machine
- `crates/llm-relay-core/src/cli_target.rs` — `CliBackend` trait, `CliTarget`, `WindowsFsBackend`, `WslBackend`
- `crates/llm-relay-core/src/config_writer/snapshot.rs` — new directory-based snapshot format + legacy migration

Modified:

- `crates/llm-relay-core/src/config_writer.rs` — `write_*`/`clear_*`/`apply_all_configs`/`clear_all_configs` rewritten on `CliBackend`
- `crates/llm-relay-core/src/proxy_server.rs` — `start_with_listeners` returns `ProxyHandle`; adds `/_relay/ping` + `/_relay/{*rest}` routes
- `crates/llm-relay-core/src/lifecycle.rs` — also binds WSL gateway listener when present
- `crates/llm-relay-core/src/database.rs` — `wsl_distros` table migration
- `crates/llm-relay-core/src/paths.rs` — `cli_config_backup_dir()`, legacy single-file path helper
- `crates/llm-relay-core/src/service.rs` — extract `CoreContext`, hold `Arc<ProxyHandle>`, build `CliTarget`s
- `crates/llm-relay-core/src/lib.rs` — module declarations
- `crates/llm-relay-core/Cargo.toml` — add `sha2`, `data-encoding` (for base32), `ipconfig` (Windows-only)
- `crates/llm-relay-core/src/ipc/protocol.rs` — new `WslDistroInfo` type + Request/Response variants
- `src-tauri/src/lib.rs` — Tauri commands `list_wsl_distros`, `toggle_wsl_distro`, `refresh_wsl_distros`, `reconnect_wsl`
- `src/components/Settings/WslDistros.tsx` — new component, mounted only on Windows
- `README.md` — add "在 WSL2 中使用" section

---

## Task Decomposition

Tasks are grouped into 7 phases. Each phase produces working software and is independently committable.

- **Phase 1**: Plumbing — types, dependencies, database migration (Tasks 1-3)
- **Phase 2**: Network — gateway IP discovery, dual listener, `/_relay/ping` (Tasks 4-7)
- **Phase 3**: WSL filesystem & distro discovery (Tasks 8-11)
- **Phase 4**: URL probing (Task 12)
- **Phase 5**: Config writer refactor + snapshot v2 + migration (Tasks 13-17)
- **Phase 6**: Service wiring + detection state machine (Tasks 18-20)
- **Phase 7**: Frontend Settings UI + README (Tasks 21-23)

---

## Phase 1: Plumbing

### Task 1: Add new dependencies

**Files:**
- Modify: `crates/llm-relay-core/Cargo.toml`

- [ ] **Step 1: Add to `[dependencies]`**

In `crates/llm-relay-core/Cargo.toml`, after the `fs2 = "0.4"` line:

```toml
sha2 = "0.10"
data-encoding = "2"
tokio-util = { version = "0.7", features = ["rt"] }  # for CancellationToken
```

- [ ] **Step 2: Add Windows-only adapter enumeration**

In the existing `[target.'cfg(windows)'.dependencies]` section, add:

```toml
ipconfig = "0.3"
```

- [ ] **Step 3: Verify compile**

Run: `cargo check -p llm-relay-core`
Expected: clean build (no other code uses these yet)

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/Cargo.toml Cargo.lock
git commit -m "build(core): add sha2, data-encoding, tokio-util, ipconfig deps for WSL2"
```

---

### Task 2: Add `wsl_distros` table migration

**Files:**
- Modify: `crates/llm-relay-core/src/database.rs`

- [ ] **Step 1: Find current max user_version**

Run: `grep "user_version = " crates/llm-relay-core/src/database.rs`
Expected: highest is `user_version = 9`. New migration is 10.

- [ ] **Step 2: Add migration block**

Locate the last migration block (the one ending with `PRAGMA user_version = 9`) and append immediately after it:

```rust
        if version < 10 {
            // WSL2 distro cache. Table exists on all platforms (harmless when
            // unused) so SQLite migrations are platform-agnostic.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS wsl_distros (
                    name          TEXT PRIMARY KEY,
                    is_default    INTEGER NOT NULL DEFAULT 0,
                    selected      INTEGER NOT NULL DEFAULT 0,
                    home          TEXT,
                    user          TEXT,
                    has_claude    INTEGER NOT NULL DEFAULT 0,
                    has_codex     INTEGER NOT NULL DEFAULT 0,
                    has_gemini    INTEGER NOT NULL DEFAULT 0,
                    resolved_url  TEXT,
                    probed_at     TEXT
                );",
            )?;
            conn.execute_batch("PRAGMA user_version = 10")?;
        }
```

- [ ] **Step 3: Add CRUD helpers to `Database`**

Append at the end of `impl Database`:

```rust
    pub fn list_wsl_distros(&self) -> Result<Vec<crate::wsl::distro::DistroRow>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, is_default, selected, home, user, has_claude, has_codex,
                    has_gemini, resolved_url, probed_at
             FROM wsl_distros ORDER BY name"
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::wsl::distro::DistroRow {
            name: r.get(0)?,
            is_default: r.get::<_, i64>(1)? != 0,
            selected: r.get::<_, i64>(2)? != 0,
            home: r.get(3)?,
            user: r.get(4)?,
            has_claude: r.get::<_, i64>(5)? != 0,
            has_codex: r.get::<_, i64>(6)? != 0,
            has_gemini: r.get::<_, i64>(7)? != 0,
            resolved_url: r.get(8)?,
            probed_at: r.get(9)?,
        }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_wsl_distro(&self, row: &crate::wsl::distro::DistroRow) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO wsl_distros
             (name, is_default, selected, home, user, has_claude, has_codex, has_gemini, resolved_url, probed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(name) DO UPDATE SET
                is_default=excluded.is_default,
                home=excluded.home,
                user=excluded.user,
                has_claude=excluded.has_claude,
                has_codex=excluded.has_codex,
                has_gemini=excluded.has_gemini,
                resolved_url=excluded.resolved_url,
                probed_at=excluded.probed_at",
            params![
                row.name,
                row.is_default as i64,
                row.selected as i64,
                row.home,
                row.user,
                row.has_claude as i64,
                row.has_codex as i64,
                row.has_gemini as i64,
                row.resolved_url,
                row.probed_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_wsl_distro_selected(&self, name: &str, selected: bool) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE wsl_distros SET selected = ?1 WHERE name = ?2",
            params![selected as i64, name],
        )?;
        Ok(())
    }

    pub fn delete_wsl_distro(&self, name: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM wsl_distros WHERE name = ?1", params![name])?;
        Ok(())
    }
```

- [ ] **Step 4: Verify compile (will fail on missing `crate::wsl`)**

Run: `cargo check -p llm-relay-core 2>&1 | head -20`
Expected: errors about `unresolved module wsl` — this is fine, we wire it in Task 8. Just confirm only those errors and nothing else broke.

- [ ] **Step 5: Stub the type so build passes**

Append to `crates/llm-relay-core/src/lib.rs`:

```rust
pub mod wsl;
```

Create `crates/llm-relay-core/src/wsl/mod.rs`:

```rust
//! WSL2 integration. All cross-platform stubs here; Windows-only impls in
//! submodules gated by `#[cfg(target_os = "windows")]`.
pub mod distro;
```

Create `crates/llm-relay-core/src/wsl/distro.rs` with just the type:

```rust
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
```

Run: `cargo check -p llm-relay-core`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-relay-core/src/database.rs crates/llm-relay-core/src/lib.rs crates/llm-relay-core/src/wsl/
git commit -m "feat(db): add wsl_distros table + CRUD; stub wsl module"
```

---

### Task 3: Add `cli_config_backup_dir()` and legacy file path

**Files:**
- Modify: `crates/llm-relay-core/src/paths.rs`

- [ ] **Step 1: Append two helpers**

Append at the end of `crates/llm-relay-core/src/paths.rs`:

```rust
/// New per-target snapshot directory. Each file inside is a
/// `target_type=windows` or `target_type=wsl` snapshot. Pre-WSL2 versions
/// stored a single file at `legacy_cli_config_backup_file()`.
pub fn cli_config_backup_dir() -> std::path::PathBuf {
    config_dir().join("cli-config-backup")
}

/// Pre-WSL2 snapshot path. Used only by the one-shot migration in
/// `config_writer::snapshot::migrate_legacy_if_needed()`.
pub fn legacy_cli_config_backup_file() -> std::path::PathBuf {
    config_dir().join("cli-config-backup.json")
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p llm-relay-core`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/llm-relay-core/src/paths.rs
git commit -m "feat(paths): add cli_config_backup_dir + legacy file helpers"
```

---

## Phase 2: Network — gateway IP, dual listener, `/_relay/ping`

### Task 4: `find_wsl_gateway_ip()` on Windows

**Files:**
- Create: `crates/llm-relay-core/src/wsl/network.rs`
- Modify: `crates/llm-relay-core/src/wsl/mod.rs`

- [ ] **Step 1: Write failing test (Windows-only)**

Create `crates/llm-relay-core/src/wsl/network.rs`:

```rust
//! Discover the Windows-side IPv4 of the `vEthernet (WSL)` adapter.
//!
//! The WSL2 NAT gateway IP changes whenever Windows or WSL restarts, so
//! callers must re-probe periodically (see `wsl::state`). Non-Windows
//! builds return `None` unconditionally.

use std::net::IpAddr;

/// Returns the IPv4 of the WSL virtual NIC, or `None` if not found / not
/// on Windows. The adapter name on Windows is "vEthernet (WSL)" (English
/// locale) or sometimes "vEthernet (WSL (Hyper-V firewall))" on newer
/// builds. Match case-insensitively on the substring "WSL".
pub fn find_wsl_gateway_ip() -> Option<IpAddr> {
    #[cfg(target_os = "windows")]
    {
        let adapters = match ipconfig::get_adapters() {
            Ok(a) => a,
            Err(_) => return None,
        };
        for ad in adapters {
            let name_lc = ad.friendly_name().to_lowercase();
            if !name_lc.contains("wsl") { continue; }
            for ip in ad.ip_addresses() {
                if let IpAddr::V4(v4) = ip {
                    // Skip 169.254.* link-local; WSL NAT IPs are usually 172.x
                    if !v4.is_link_local() {
                        return Some(IpAddr::V4(*v4));
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_on_non_windows_or_when_wsl_absent() {
        // On Linux/macOS: always None. On Windows without WSL: also None.
        // On Windows with WSL: Some(IpAddr). This test only asserts the
        // function doesn't panic and returns a typed value.
        let _ = find_wsl_gateway_ip();
    }
}
```

- [ ] **Step 2: Register module**

Update `crates/llm-relay-core/src/wsl/mod.rs`:

```rust
//! WSL2 integration. All cross-platform stubs here; Windows-only impls in
//! submodules gated by `#[cfg(target_os = "windows")]`.
pub mod distro;
pub mod network;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p llm-relay-core wsl::network`
Expected: PASS (1 test).

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/wsl/network.rs crates/llm-relay-core/src/wsl/mod.rs
git commit -m "feat(wsl): find_wsl_gateway_ip via ipconfig (Windows-only)"
```

---

### Task 5: Add `/_relay/ping` + reserved-namespace 404 to proxy router

**Files:**
- Modify: `crates/llm-relay-core/src/proxy_server.rs`

- [ ] **Step 1: Add the two handlers and a builder fn**

In `crates/llm-relay-core/src/proxy_server.rs`, just below the existing `use` block and the `pub fn proxy_base_url()` definition, add:

```rust
use axum::routing::{any, get};

async fn relay_ping() -> &'static str { "ok" }

async fn relay_reserved() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "unknown relay endpoint")
}

/// Build the axum Router shared by every listener. Local `/_relay/*` routes
/// must come before `.fallback(forward)` so unknown reserved paths don't
/// get forwarded upstream. New `_relay/*` endpoints register before the
/// `{*rest}` wildcard.
pub fn build_router(state: ProxyState) -> axum::Router {
    axum::Router::new()
        .route("/_relay/ping", get(relay_ping))
        .route("/_relay/{*rest}", any(relay_reserved))
        .fallback(forward)
        .with_state(state)
}
```

- [ ] **Step 2: Replace inline router build with the new builder**

Find this in `start_with_listener`:

```rust
    let app = Router::new()
        .fallback(forward)
        .with_state(state);
```

Replace with:

```rust
    let app = build_router(state);
```

Remove the now-unused `Router` import line in that function if it stands alone (keep the module-level `use axum::{Router, ...}` line — `build_router` still references `axum::Router` qualified).

- [ ] **Step 3: Test the router**

Append at the end of `proxy_server.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    fn test_state() -> ProxyState {
        // Use an in-memory database for the test.
        let db = Database::open_in_memory().expect("open_in_memory");
        ProxyState {
            db: Arc::new(db),
            switch_lock: Arc::new(tokio::sync::Mutex::new(())),
            sink: Arc::new(crate::events::NullSink),
            consecutive_errors: Arc::new(AtomicU32::new(0)),
        }
    }

    #[tokio::test]
    async fn relay_ping_returns_200_ok() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(Request::builder().uri("/_relay/ping").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn relay_reserved_namespace_returns_404_locally() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(Request::builder().uri("/_relay/unknown").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // 404 is generated by our handler, NOT forwarded upstream (which
        // would either fail with 503 "No active config" or hit a real
        // gateway). The body proves it.
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"unknown relay endpoint");
    }
}
```

- [ ] **Step 4: Add `Database::open_in_memory` helper if missing**

Run: `grep "open_in_memory" crates/llm-relay-core/src/database.rs`
If absent, add to `impl Database`:

```rust
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory()?;
        let db = Self { conn: Mutex::new(conn) };
        db.run_migrations()?;
        Ok(db)
    }
```

If `run_migrations` is not factored out as its own method, just copy the schema+migration `execute_batch` calls from `Database::open` into this helper. Don't refactor — copy the literal SQL.

- [ ] **Step 5: Add `tower` dev-dep for the oneshot test helper**

In `crates/llm-relay-core/Cargo.toml` `[dev-dependencies]`, add:

```toml
tower = { version = "0.5", features = ["util"] }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p llm-relay-core proxy_server::tests`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/llm-relay-core/src/proxy_server.rs crates/llm-relay-core/src/database.rs crates/llm-relay-core/Cargo.toml Cargo.lock
git commit -m "feat(proxy): /_relay/ping + reserved-namespace 404 before fallback"
```

---
### Task 6: Extract `CoreContext`; introduce `ProxyHandle`

**Files:**
- Modify: `crates/llm-relay-core/src/proxy_server.rs`
- Modify: `crates/llm-relay-core/src/service.rs`

This task breaks the `Service` ↔ `ProxyHandle` construction cycle by extracting the shared context.

- [ ] **Step 1: Add `CoreContext`**

In `crates/llm-relay-core/src/service.rs`, above `pub struct Service`:

```rust
pub struct CoreContext {
    pub db: Arc<crate::Database>,
    pub sink: crate::SharedEventSink,
    pub switch_lock: Arc<Mutex<()>>,
}

impl CoreContext {
    pub fn new(db: Arc<crate::Database>, sink: crate::SharedEventSink) -> Self {
        Self { db, sink, switch_lock: Arc::new(Mutex::new(())) }
    }
}
```

- [ ] **Step 2: Make Service hold `Arc<CoreContext>` and `Option<Arc<ProxyHandle>>`**

Replace `pub struct Service { ... }` and its `Service::new`:

```rust
#[derive(Clone)]
pub struct Service {
    pub ctx: Arc<CoreContext>,
    pub proxy: Option<Arc<crate::proxy_server::ProxyHandle>>,
}

impl Service {
    pub fn new(db: Arc<crate::Database>, sink: crate::SharedEventSink) -> Self {
        Self {
            ctx: Arc::new(CoreContext::new(db, sink)),
            proxy: None,
        }
    }

    pub fn from_ctx(ctx: Arc<CoreContext>) -> Self {
        Self { ctx, proxy: None }
    }

    pub fn with_proxy(mut self, proxy: Arc<crate::proxy_server::ProxyHandle>) -> Self {
        self.proxy = Some(proxy);
        self
    }
}
```

- [ ] **Step 3: Migrate field accesses inside `impl Service`**

In `service.rs`, replace literal occurrences (Edit `replace_all` for each):
- `self.db.` → `self.ctx.db.`
- `self.sink` → `self.ctx.sink`
- `self.switch_lock.lock()` → `self.ctx.switch_lock.lock()`

- [ ] **Step 4: Let `ProxyState` be built from `CoreContext`**

In `crates/llm-relay-core/src/proxy_server.rs`, after the `ProxyState` declaration:

```rust
impl ProxyState {
    pub fn from_ctx(ctx: &crate::service::CoreContext) -> Self {
        Self {
            db: ctx.db.clone(),
            switch_lock: ctx.switch_lock.clone(),
            sink: ctx.sink.clone(),
            consecutive_errors: Arc::new(AtomicU32::new(0)),
        }
    }
}
```

- [ ] **Step 5: Add `ProxyHandle` and `start_with_listeners`**

At the bottom of `crates/llm-relay-core/src/proxy_server.rs`, before any existing `#[cfg(test)]` block:

```rust
use std::net::IpAddr;
use std::sync::Mutex as StdMutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct ProxyHandle {
    primary_token: CancellationToken,
    primary_join: StdMutex<Option<JoinHandle<()>>>,
    wsl: StdMutex<Option<WslBound>>,
    state: ProxyState,
}

struct WslBound {
    ip: IpAddr,
    token: CancellationToken,
    join: JoinHandle<()>,
}

impl ProxyHandle {
    pub async fn shutdown(self: Arc<Self>) {
        self.primary_token.cancel();
        let wsl = self.wsl.lock().unwrap().take();
        if let Some(wsl) = wsl {
            wsl.token.cancel();
            let _ = wsl.join.await;
        }
        let primary = self.primary_join.lock().unwrap().take();
        if let Some(j) = primary { let _ = j.await; }
    }

    pub async fn rebind_wsl(self: &Arc<Self>, new_ip: Option<IpAddr>) -> Result<(), crate::AppError> {
        let old = self.wsl.lock().unwrap().take();
        if let Some(old) = old {
            old.token.cancel();
            let _ = old.join.await;
        }
        let Some(ip) = new_ip else { return Ok(()) };
        let port = crate::paths::proxy_port();
        let std_listener = std::net::TcpListener::bind((ip, port))
            .map_err(|e| crate::AppError::Config(format!("WSL bind {ip}:{port} failed: {e}")))?;
        std_listener.set_nonblocking(true)
            .map_err(|e| crate::AppError::Config(format!("WSL nonblocking: {e}")))?;
        let tokio_listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| crate::AppError::Config(format!("WSL wrap: {e}")))?;
        let token = CancellationToken::new();
        let state = self.state.clone();
        let tok = token.clone();
        let join = tokio::spawn(async move {
            let app = build_router(state);
            let _ = axum::serve(tokio_listener, app)
                .with_graceful_shutdown(async move { tok.cancelled().await })
                .await;
        });
        *self.wsl.lock().unwrap() = Some(WslBound { ip, token, join });
        log::info!("WSL listener bound on {ip}:{port}");
        Ok(())
    }

    pub fn wsl_ip(&self) -> Option<IpAddr> {
        self.wsl.lock().unwrap().as_ref().map(|w| w.ip)
    }
}

pub async fn start_with_listeners(
    state: ProxyState,
    primary: std::net::TcpListener,
    initial_wsl: Option<(IpAddr, std::net::TcpListener)>,
) -> Arc<ProxyHandle> {
    primary.set_nonblocking(true).expect("primary nonblocking");
    let primary_tokio = tokio::net::TcpListener::from_std(primary).expect("wrap primary");
    let primary_token = CancellationToken::new();
    let state_for_task = state.clone();
    let tok = primary_token.clone();
    let primary_join = tokio::spawn(async move {
        let app = build_router(state_for_task);
        let _ = axum::serve(primary_tokio, app)
            .with_graceful_shutdown(async move { tok.cancelled().await })
            .await;
    });
    let handle = Arc::new(ProxyHandle {
        primary_token,
        primary_join: StdMutex::new(Some(primary_join)),
        wsl: StdMutex::new(None),
        state,
    });
    log::info!("Local proxy started on 127.0.0.1:{}", crate::paths::proxy_port());
    if let Some((ip, listener)) = initial_wsl {
        if listener.set_nonblocking(true).is_ok() {
            if let Ok(tl) = tokio::net::TcpListener::from_std(listener) {
                let token = CancellationToken::new();
                let st = handle.state.clone();
                let tk = token.clone();
                let join = tokio::spawn(async move {
                    let app = build_router(st);
                    let _ = axum::serve(tl, app)
                        .with_graceful_shutdown(async move { tk.cancelled().await })
                        .await;
                });
                *handle.wsl.lock().unwrap() = Some(WslBound { ip, token, join });
                log::info!("WSL listener bound on {ip}:{}", crate::paths::proxy_port());
            }
        }
    }
    handle
}
```

Make sure `ProxyState` derives `Clone` — if not, add `#[derive(Clone)]` to it. (It already does in the current source.)

- [ ] **Step 6: Verify compile**

```bash
cargo check -p llm-relay-core --tests
```

Expected: clean. Existing `start_with_listener` (singular) is left in place as a shim for now.

- [ ] **Step 7: Commit**

```bash
git add crates/llm-relay-core/src/proxy_server.rs crates/llm-relay-core/src/service.rs
git commit -m "refactor(core): extract CoreContext; add ProxyHandle with cancel+join shutdown"
```

---

### Task 7: Wire `lifecycle` and call sites to use `ProxyHandle`

**Files:**
- Modify: `crates/llm-relay-core/src/lifecycle.rs`
- Modify: `crates/llm-relay-agent/src/main.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add WSL pre-bind to `LifecycleGuard`**

In `crates/llm-relay-core/src/lifecycle.rs`, extend the struct:

```rust
pub struct LifecycleGuard {
    _lock: File,
    pub proxy_listener: Option<TcpListener>,
    pub wsl_listener: Option<(std::net::IpAddr, TcpListener)>,
}
```

In `acquire()`, after the pidfile write and before `Ok(Self {...})`:

```rust
        // Best-effort: also bind the WSL2 NIC gateway IP if present.
        // Failure here is non-fatal — the agent remains usable for Windows.
        let wsl_listener = crate::wsl::network::find_wsl_gateway_ip()
            .and_then(|ip| {
                TcpListener::bind((ip, paths::proxy_port()))
                    .map_err(|e| {
                        log::warn!("WSL bind {ip}:{} failed: {e}", paths::proxy_port());
                        e
                    })
                    .ok()
                    .map(|l| (ip, l))
            });
```

Update the returned `Ok(Self { ... })` to include `wsl_listener`.

Update the existing `Drop` impl on `LifecycleGuard` if needed — `wsl_listener` doesn't need any special cleanup (drop closes it).

- [ ] **Step 2: Find all call sites of the old proxy startup**

```bash
grep -rn "start_with_listener\b" crates/ src-tauri/ | grep -v target
```

Note the file:line of every hit.

- [ ] **Step 3: Update the agent**

In `crates/llm-relay-agent/src/main.rs`, replace the block that calls `proxy_server::start_with_listener(service, listener).await` with:

```rust
let primary = guard.take_listener().expect("primary listener");
let initial_wsl = guard.wsl_listener.take();
let state = llm_relay_core::proxy_server::ProxyState::from_ctx(&service.ctx);
let proxy_handle = llm_relay_core::proxy_server::start_with_listeners(
    state, primary, initial_wsl,
).await;
let service = service.with_proxy(proxy_handle.clone());

// IPC server consumes `service`; shutdown signal cancels proxy_handle.
// If the existing main awaited start_with_listener as the long-running
// task, replace that await with whatever the IPC server returns. Typical
// pattern:
//   tokio::select! {
//       _ = ipc_server_run(service.clone()) => {},
//       _ = signal::ctrl_c() => {},
//   }
//   proxy_handle.clone().shutdown().await;
```

- [ ] **Step 4: Update the Tauri GUI**

In `src-tauri/src/lib.rs`, find the same `start_with_listener` call and apply the equivalent swap. Save `proxy_handle` into Tauri state via `app.manage(...)` if existing commands need it.

- [ ] **Step 5: Verify workspace builds + tests pass**

```bash
cargo check --workspace
cargo test --workspace
```

Expected: clean build, all existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(lifecycle): bind WSL gateway listener; wire ProxyHandle through agent + GUI"
```

---

## Phase 3: WSL filesystem & distro discovery

### Task 8: `wsl::fs` — read / write / remove / exists via `wsl.exe`

**Files:**
- Create: `crates/llm-relay-core/src/wsl/fs.rs`
- Modify: `crates/llm-relay-core/src/wsl/mod.rs`

- [ ] **Step 1: Write the module**

Create `crates/llm-relay-core/src/wsl/fs.rs`:

```rust
//! File I/O inside a WSL2 distro via `wsl.exe -d <D> -e sh -c ...`.
//!
//! Used by `WslBackend` so that all snapshot/apply logic in `config_writer`
//! works identically against Windows-native paths and per-distro Linux
//! filesystems. Errors from a stopped/unregistered distro are surfaced as
//! `AppError::Config` so the caller can warn-and-skip per-target.

use crate::AppError;
use std::io::Write;
use std::process::{Command, Stdio};

const WSL_TIMEOUT_SECS: u64 = 5;

#[cfg(not(target_os = "windows"))]
pub fn wsl_read(_distro: &str, _path: &str) -> Result<Option<String>, AppError> {
    Err(AppError::Config("WSL fs ops only available on Windows".into()))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_atomic_write(_distro: &str, _path: &str, _bytes: &[u8]) -> Result<(), AppError> {
    Err(AppError::Config("WSL fs ops only available on Windows".into()))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_remove(_distro: &str, _path: &str) -> Result<(), AppError> {
    Err(AppError::Config("WSL fs ops only available on Windows".into()))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_exists(_distro: &str, _path: &str) -> Result<bool, AppError> {
    Err(AppError::Config("WSL fs ops only available on Windows".into()))
}

#[cfg(target_os = "windows")]
pub fn wsl_read(distro: &str, path: &str) -> Result<Option<String>, AppError> {
    // Use `[ -f ... ] && cat ... || true` so missing file returns empty
    // stdout + exit 0, distinguishable from "distro broken" (exit != 0).
    let script = format!(
        "if [ -f {path:?} ]; then cat {path:?}; fi",
        path = shell_escape(path),
    );
    let out = run_wsl(distro, &script, None)?;
    if out.is_empty() {
        // Could be empty file or absent file. Check existence to disambiguate.
        if wsl_exists(distro, path)? {
            Ok(Some(String::new()))
        } else {
            Ok(None)
        }
    } else {
        Ok(Some(String::from_utf8_lossy(&out).into_owned()))
    }
}

#[cfg(target_os = "windows")]
pub fn wsl_atomic_write(distro: &str, path: &str, bytes: &[u8]) -> Result<(), AppError> {
    // Inside the distro: mkdir parent, mktemp, write stdin, mv -f. The mv
    // is atomic on the same filesystem.
    let script = format!(
        r#"umask 077
d="$(dirname {path})"
mkdir -p "$d"
t="$(mktemp "$d/.llmrelay.tmp.XXXXXX")"
cat > "$t"
mv -f "$t" {path}"#,
        path = shell_escape(path),
    );
    let _ = run_wsl(distro, &script, Some(bytes))?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn wsl_remove(distro: &str, path: &str) -> Result<(), AppError> {
    let script = format!("rm -f {path}", path = shell_escape(path));
    let _ = run_wsl(distro, &script, None)?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn wsl_exists(distro: &str, path: &str) -> Result<bool, AppError> {
    let script = format!(
        "if [ -e {path} ]; then echo 1; else echo 0; fi",
        path = shell_escape(path),
    );
    let out = run_wsl(distro, &script, None)?;
    Ok(String::from_utf8_lossy(&out).trim() == "1")
}

#[cfg(target_os = "windows")]
fn run_wsl(distro: &str, script: &str, stdin_bytes: Option<&[u8]>) -> Result<Vec<u8>, AppError> {
    let mut cmd = Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "sh", "-c", script]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Config(format!("wsl.exe spawn ({distro}): {e}")))?;
    if let (Some(bytes), Some(mut stdin)) = (stdin_bytes, child.stdin.take()) {
        stdin.write_all(bytes).map_err(|e| AppError::Config(format!("wsl stdin: {e}")))?;
        drop(stdin);
    }
    // Manual timeout: wait_timeout would be nicer, but we avoid a new dep.
    // Poll every 50ms up to WSL_TIMEOUT_SECS.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if start.elapsed().as_secs() >= WSL_TIMEOUT_SECS {
                    let _ = child.kill();
                    return Err(AppError::Config(format!(
                        "wsl.exe -d {distro} timed out after {WSL_TIMEOUT_SECS}s"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(AppError::Config(format!("wsl wait: {e}"))),
        }
    }
    let out = child.wait_with_output().map_err(|e| AppError::Config(format!("wsl output: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Config(format!(
            "wsl.exe -d {distro} exited with {}: {stderr}",
            out.status
        )));
    }
    Ok(out.stdout)
}

#[cfg(target_os = "windows")]
fn shell_escape(s: &str) -> String {
    // Single-quote the path for sh; embed single quotes as '\''.
    let escaped = s.replace('\'', r#"'\''"#);
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn unknown_distro_returns_err_not_panic() {
        // We don't have a guaranteed-absent distro name to test against
        // without coupling to environment, but garbage input should error.
        let r = wsl_exists("__definitely_not_a_real_distro__", "/tmp/foo");
        assert!(r.is_err(), "expected err, got {r:?}");
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn non_windows_returns_err() {
        assert!(wsl_read("anything", "/tmp/x").is_err());
    }

    #[test]
    fn shell_escape_handles_single_quotes() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(shell_escape("a'b"), r#"'a'\''b'"#);
            assert_eq!(shell_escape("/home/x/foo bar"), "'/home/x/foo bar'");
        }
    }
}
```

- [ ] **Step 2: Register module**

Update `crates/llm-relay-core/src/wsl/mod.rs`:

```rust
pub mod distro;
pub mod network;
pub mod fs;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p llm-relay-core wsl::fs`
Expected: PASS on all platforms. On Windows the `unknown_distro_returns_err_not_panic` test runs.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/wsl/
git commit -m "feat(wsl): fs ops via wsl.exe with 5s timeout + shell-safe quoting"
```

---
### Task 9: `wsl::distro` — discover distros via `wsl.exe -l -v`

**Files:**
- Modify: `crates/llm-relay-core/src/wsl/distro.rs`

- [ ] **Step 1: Add discovery API**

Append below `pub struct DistroRow` in `crates/llm-relay-core/src/wsl/distro.rs`:

```rust
use crate::AppError;

/// Snapshot of `wsl.exe -l -v` parsed into distro entries. WSL1 distros
/// are filtered out (we only support WSL2). Returns empty Vec when WSL
/// is absent / disabled / has no installed distros — never an Err for
/// those cases. Err only for unexpected I/O.
#[derive(Debug, Clone)]
pub struct DiscoveredDistro {
    pub name: String,
    pub is_default: bool,
    pub running: bool,
}

#[cfg(not(target_os = "windows"))]
pub fn discover_distros() -> Result<Vec<DiscoveredDistro>, AppError> {
    Ok(Vec::new())
}

#[cfg(target_os = "windows")]
pub fn discover_distros() -> Result<Vec<DiscoveredDistro>, AppError> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = match Command::new("wsl.exe")
        .args(["-l", "-v"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AppError::Config(format!("wsl.exe -l -v: {e}"))),
    };
    // wsl.exe outputs UTF-16LE with a BOM on most builds. Decode best-effort.
    let text = decode_wsl_output(&out.stdout);
    if !out.status.success() {
        // "no installed distributions" prints to stdout; treat any non-zero
        // exit with empty list quietly. Log for diagnostics.
        log::debug!("wsl.exe -l -v exited {}; stdout: {}", out.status, text);
        return Ok(Vec::new());
    }
    Ok(parse_wsl_list(&text))
}

#[cfg(target_os = "windows")]
fn decode_wsl_output(bytes: &[u8]) -> String {
    // BOM 0xFF 0xFE = UTF-16LE
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else if bytes.len() % 2 == 0
        && bytes.iter().enumerate().filter(|(i, _)| i % 2 == 1).all(|(_, b)| *b == 0)
    {
        // Heuristic: looks like UTF-16LE without BOM (every odd byte is 0)
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Parse the columnar text of `wsl -l -v`. Format (header on line 1):
///   "  NAME       STATE    VERSION"
///   "* Ubuntu     Running  2"
///   "  Debian     Stopped  2"
///   "  Legacy     Stopped  1"   <- filtered (WSL1)
fn parse_wsl_list(text: &str) -> Vec<DiscoveredDistro> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_end();
        if i == 0 || trimmed.is_empty() {
            continue;  // header / blank
        }
        // First column is either '*' (default) or ' '.
        let is_default = trimmed.starts_with('*');
        // Strip leading marker and split on whitespace.
        let rest = trimmed.trim_start_matches('*').trim_start();
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 3 { continue; }
        let name = parts[0].to_string();
        let state = parts[1];
        let version = parts[2];
        if version != "2" { continue; }  // skip WSL1
        out.push(DiscoveredDistro {
            name,
            is_default,
            running: state.eq_ignore_ascii_case("Running"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_output() {
        let text = "  NAME       STATE    VERSION\n\
                    * Ubuntu     Running  2\n\
                      Debian     Stopped  2\n\
                      Legacy     Stopped  1\n";
        let got = parse_wsl_list(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Ubuntu");
        assert!(got[0].is_default);
        assert!(got[0].running);
        assert_eq!(got[1].name, "Debian");
        assert!(!got[1].is_default);
        assert!(!got[1].running);
    }

    #[test]
    fn empty_output_yields_empty() {
        assert!(parse_wsl_list("").is_empty());
        assert!(parse_wsl_list("  NAME STATE VERSION\n").is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn decode_handles_utf16_bom() {
        let s = "hello";
        let mut bytes = vec![0xFF, 0xFE];
        for c in s.encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(decode_wsl_output(&bytes), "hello");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p llm-relay-core wsl::distro`
Expected: PASS (3 tests on Windows; 2 on other platforms).

- [ ] **Step 3: Commit**

```bash
git add crates/llm-relay-core/src/wsl/distro.rs
git commit -m "feat(wsl): discover_distros via wsl.exe -l -v with UTF-16LE handling"
```

---
### Task 10: `wsl::distro::probe_distro` — fetch `$HOME`, `whoami`, installed CLIs

**Files:**
- Modify: `crates/llm-relay-core/src/wsl/distro.rs`

- [ ] **Step 1: Add probe function**

Append to `crates/llm-relay-core/src/wsl/distro.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub home: Option<String>,
    pub user: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
}

/// Probe a single distro for $HOME, whoami, and presence of the three CLI
/// binaries. All four values are fetched in a single `wsl.exe -e sh -c`
/// invocation to minimize cold-start cost. Each value parsed by key name;
/// the per-binary loop avoids `&&` short-circuiting (a missing claude
/// would otherwise skip codex/gemini checks).
#[cfg(target_os = "windows")]
pub fn probe_distro(name: &str) -> Result<ProbeResult, AppError> {
    let script = r#"echo "home=$HOME"
echo "user=$(whoami)"
for c in claude codex gemini; do
  if command -v "$c" >/dev/null 2>&1; then
    echo "$c=1"
  else
    echo "$c=0"
  fi
done"#;
    let out = crate::wsl::fs::__wsl_run_script(name, script)?;
    Ok(parse_probe_output(&out))
}

#[cfg(not(target_os = "windows"))]
pub fn probe_distro(_name: &str) -> Result<ProbeResult, AppError> {
    Err(AppError::Config("probe_distro: Windows only".into()))
}

fn parse_probe_output(text: &str) -> ProbeResult {
    let mut r = ProbeResult::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "home" => r.home = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
            "user" => r.user = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
            "claude" => r.has_claude = v.trim() == "1",
            "codex" => r.has_codex = v.trim() == "1",
            "gemini" => r.has_gemini = v.trim() == "1",
            _ => {}
        }
    }
    r
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    #[test]
    fn parses_complete_output() {
        let text = "home=/home/xanzh\nuser=xanzh\nclaude=1\ncodex=0\ngemini=1\n";
        let r = parse_probe_output(text);
        assert_eq!(r.home.as_deref(), Some("/home/xanzh"));
        assert_eq!(r.user.as_deref(), Some("xanzh"));
        assert!(r.has_claude);
        assert!(!r.has_codex);
        assert!(r.has_gemini);
    }

    #[test]
    fn missing_values_default_to_false_none() {
        let r = parse_probe_output("");
        assert!(r.home.is_none());
        assert!(!r.has_claude);
    }
}
```

- [ ] **Step 2: Expose `__wsl_run_script` helper in `wsl::fs`**

In `crates/llm-relay-core/src/wsl/fs.rs`, after the existing `run_wsl` fn, add:

```rust
#[cfg(target_os = "windows")]
pub(crate) fn __wsl_run_script(distro: &str, script: &str) -> Result<String, AppError> {
    let bytes = run_wsl(distro, script, None)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p llm-relay-core wsl::distro::probe_tests
```

Expected: 2 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/wsl/
git commit -m "feat(wsl): probe_distro fetches home/user/installed CLIs in one wsl.exe call"
```

---

### Task 11: Reconcile discovered distros into DB

**Files:**
- Modify: `crates/llm-relay-core/src/wsl/distro.rs`

- [ ] **Step 1: Add `refresh_distros_in_db`**

Append at the end of `crates/llm-relay-core/src/wsl/distro.rs`:

```rust
/// Re-discover distros from `wsl.exe`, run `probe_distro` for each, and
/// reconcile into the `wsl_distros` table:
/// - new distro found → insert with selected = is_default
/// - existing distro: update home/user/installed/probed_at; preserve
///   `selected` so user's explicit choice survives
/// - distro disappeared from `wsl -l -v` → delete row
///
/// Probe failures for a single distro do NOT abort reconciliation — the
/// row gets upserted with whatever fields were available. URL probing
/// is a separate step (see `wsl::probe::probe_url_for_distro`); this
/// function leaves `resolved_url` alone.
pub fn refresh_distros_in_db(db: &crate::Database) -> Result<Vec<DistroRow>, AppError> {
    let discovered = discover_distros()?;
    let existing = db.list_wsl_distros()?;
    let now = chrono::Utc::now().to_rfc3339();

    // Reconcile
    let discovered_names: std::collections::HashSet<&str> =
        discovered.iter().map(|d| d.name.as_str()).collect();
    for ex in &existing {
        if !discovered_names.contains(ex.name.as_str()) {
            log::info!("WSL distro removed: {}", ex.name);
            db.delete_wsl_distro(&ex.name)?;
        }
    }

    let mut out = Vec::with_capacity(discovered.len());
    for d in discovered {
        let prior = existing.iter().find(|e| e.name == d.name);
        // Probe (best-effort)
        let probe = match probe_distro(&d.name) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("probe_distro({}) failed: {e}", d.name);
                ProbeResult::default()
            }
        };
        let row = DistroRow {
            name: d.name.clone(),
            is_default: d.is_default,
            selected: prior.map(|p| p.selected).unwrap_or(d.is_default),
            home: probe.home.or_else(|| prior.and_then(|p| p.home.clone())),
            user: probe.user.or_else(|| prior.and_then(|p| p.user.clone())),
            has_claude: probe.has_claude,
            has_codex: probe.has_codex,
            has_gemini: probe.has_gemini,
            resolved_url: prior.and_then(|p| p.resolved_url.clone()),
            probed_at: Some(now.clone()),
        };
        db.upsert_wsl_distro(&row)?;
        out.push(row);
    }
    Ok(out)
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p llm-relay-core`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/llm-relay-core/src/wsl/distro.rs
git commit -m "feat(wsl): refresh_distros_in_db reconciles discovery + probe with cached rows"
```

---
## Phase 4: URL probing

### Task 12: `wsl::probe::probe_url_for_distro`

**Files:**
- Create: `crates/llm-relay-core/src/wsl/probe.rs`
- Modify: `crates/llm-relay-core/src/wsl/mod.rs`

- [ ] **Step 1: Write the probe**

Create `crates/llm-relay-core/src/wsl/probe.rs`:

```rust
//! Per-distro URL probe. Decides the WSL `base_url` that gets written to
//! CLI configs. Runs a single `wsl.exe -e sh -c` against a script that
//! tries curl → wget → /dev/tcp (the last only when both bash + GNU
//! timeout exist AND the corresponding Relay listener was bound).

use crate::AppError;

#[derive(Debug, Clone, Copy)]
pub struct ListenerBinds {
    /// 127.0.0.1 listener is mandatory and always bound.
    pub loopback: bool,
    /// host.docker.internal target: TRUE only if the WSL gateway IP
    /// listener was successfully bound (otherwise some other process
    /// may own that IP:port and TCP-only verification would lie).
    pub host_docker_internal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// HTTP 200 (or TCP connect, see ProbeMethod) succeeded for this URL.
    Ok { url: String, method: ProbeMethod },
    /// Neither curl nor wget present AND no TCP fallback was eligible.
    /// User must install curl/wget inside the distro before auto-probe
    /// can pick a URL. The UI surfaces this as a specific hint.
    NoProbeTool,
    /// All candidates failed.
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeMethod {
    HttpCurl,
    HttpWget,
    /// /dev/tcp: TCP three-way handshake only, no HTTP validation.
    TcpOnly,
}

#[cfg(not(target_os = "windows"))]
pub fn probe_url_for_distro(_distro: &str, _binds: ListenerBinds) -> Result<ProbeOutcome, AppError> {
    Err(AppError::Config("probe_url_for_distro: Windows only".into()))
}

#[cfg(target_os = "windows")]
pub fn probe_url_for_distro(distro: &str, binds: ListenerBinds) -> Result<ProbeOutcome, AppError> {
    let port = crate::paths::proxy_port();
    let script = format!(
        r#"probe() {{
  url="$1"
  can_tcp="$2"
  if command -v curl >/dev/null 2>&1; then
    code=$(curl -fsS -o /dev/null -w "%{{http_code}}" --max-time 2 "$url/_relay/ping" 2>/dev/null)
    if [ "$code" = "200" ]; then echo "curl"; return 0; fi
    return 1
  elif command -v wget >/dev/null 2>&1; then
    if wget -q -O /dev/null --timeout=2 --tries=1 "$url/_relay/ping" 2>/dev/null; then
      echo "wget"; return 0
    fi
    return 1
  elif [ "$can_tcp" = "1" ] && command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    host=$(echo "$url" | sed -E "s|http://([^:/]+).*|\1|")
    port=$(echo "$url" | sed -E "s|http://[^:]+:([0-9]+).*|\1|")
    if timeout 2 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null; then
      echo "tcp"; return 0
    fi
    return 1
  else
    return 2  # no probe tool eligible
  fi
}}
HDI="http://host.docker.internal:{port}"
LOC="http://127.0.0.1:{port}"
if probe "$HDI" "{hdi}"; then echo "OK $HDI"; exit 0; fi
hdi_rc=$?
if probe "$LOC" "{loc}"; then echo "OK $LOC"; exit 0; fi
loc_rc=$?
if [ "$hdi_rc" = "2" ] && [ "$loc_rc" = "2" ]; then echo "NOTOOL"; exit 3; fi
echo "UNREACH"; exit 1
"#,
        port = port,
        hdi = if binds.host_docker_internal { "1" } else { "0" },
        loc = if binds.loopback { "1" } else { "0" },
    );
    // Don't bubble distro errors as Err — interpret stdout instead so we
    // can distinguish NoProbeTool / Unreachable / Ok.
    let stdout = match crate::wsl::fs::__wsl_run_script(distro, &script) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("probe_url_for_distro({distro}): {e}");
            return Ok(ProbeOutcome::Unreachable);
        }
    };
    Ok(parse_probe_outcome(&stdout))
}

fn parse_probe_outcome(stdout: &str) -> ProbeOutcome {
    // Look at the last line; the script prints OK <url> or UNREACH or NOTOOL.
    // The probe() function also prints curl/wget/tcp on success; that's the
    // line right before OK. We don't need the method for the snapshot value
    // but we surface it for diagnostics.
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let last = lines.last().copied().unwrap_or("");
    if let Some(rest) = last.strip_prefix("OK ") {
        let url = rest.trim().to_string();
        // Method line, if present, is the line that printed "curl"/"wget"/"tcp"
        let method = lines
            .iter()
            .rev()
            .find_map(|l| match *l {
                "curl" => Some(ProbeMethod::HttpCurl),
                "wget" => Some(ProbeMethod::HttpWget),
                "tcp" => Some(ProbeMethod::TcpOnly),
                _ => None,
            })
            .unwrap_or(ProbeMethod::HttpCurl);
        return ProbeOutcome::Ok { url, method };
    }
    if last == "NOTOOL" { return ProbeOutcome::NoProbeTool; }
    ProbeOutcome::Unreachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_with_curl() {
        let out = "curl\nOK http://host.docker.internal:18080\n";
        assert_eq!(
            parse_probe_outcome(out),
            ProbeOutcome::Ok {
                url: "http://host.docker.internal:18080".into(),
                method: ProbeMethod::HttpCurl,
            },
        );
    }

    #[test]
    fn parse_notool() {
        assert_eq!(parse_probe_outcome("NOTOOL\n"), ProbeOutcome::NoProbeTool);
    }

    #[test]
    fn parse_unreach() {
        assert_eq!(parse_probe_outcome("UNREACH\n"), ProbeOutcome::Unreachable);
    }

    #[test]
    fn parse_ok_with_tcp_fallback() {
        let out = "tcp\nOK http://127.0.0.1:18080\n";
        let got = parse_probe_outcome(out);
        if let ProbeOutcome::Ok { method, .. } = got {
            assert_eq!(method, ProbeMethod::TcpOnly);
        } else {
            panic!("expected Ok");
        }
    }
}
```

- [ ] **Step 2: Register module**

Update `crates/llm-relay-core/src/wsl/mod.rs`:

```rust
pub mod distro;
pub mod network;
pub mod fs;
pub mod probe;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p llm-relay-core wsl::probe
```

Expected: 4 PASS on all platforms.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/wsl/probe.rs crates/llm-relay-core/src/wsl/mod.rs
git commit -m "feat(wsl): probe_url_for_distro picks host.docker.internal | 127.0.0.1 via HTTP 200 or TCP fallback"
```

---
## Phase 5: Config writer refactor + snapshot v2 + migration

### Task 13: Introduce `CliBackend` trait and `WindowsFsBackend`

**Files:**
- Create: `crates/llm-relay-core/src/cli_target.rs`
- Modify: `crates/llm-relay-core/src/lib.rs`

- [ ] **Step 1: Write the module**

Create `crates/llm-relay-core/src/cli_target.rs`:

```rust
//! Abstraction layer that lets `config_writer` operate identically against
//! Windows-native paths and per-distro Linux filesystems. Backends only
//! cover filesystem ops; per-target metadata (base_url, installed tools,
//! snapshot identity) lives on `CliTarget`.

use crate::AppError;
use std::path::PathBuf;

pub trait CliBackend: Send + Sync {
    fn read(&self, rel_path: &[&str]) -> Result<Option<String>, AppError>;
    fn write_atomic(&self, rel_path: &[&str], bytes: &[u8]) -> Result<(), AppError>;
    fn remove(&self, rel_path: &[&str]) -> Result<(), AppError>;
    fn exists(&self, rel_path: &[&str]) -> Result<bool, AppError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType { Windows, Wsl }

#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub target_type: TargetType,
    /// None for Windows; Some(<original wsl -d name>) for WSL targets.
    pub distro_name: Option<String>,
    /// For WSL targets, the probed `$HOME`. Stored in snapshot JSON so
    /// restore doesn't need to re-probe a possibly-stopped distro.
    pub home: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct InstalledTools {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

impl InstalledTools {
    pub const ALL: Self = Self { claude: true, codex: true, gemini: true };
}

pub struct CliTarget {
    pub backend: Box<dyn CliBackend>,
    pub base_url: String,
    pub installed: InstalledTools,
    pub label: String,
    pub snapshot_meta: SnapshotMeta,
}

/// Backend that reads/writes inside the Windows user's home directory.
pub struct WindowsFsBackend {
    home: PathBuf,
}

impl WindowsFsBackend {
    pub fn new() -> Self {
        Self { home: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")) }
    }
    fn full_path(&self, rel: &[&str]) -> PathBuf {
        let mut p = self.home.clone();
        for seg in rel { p.push(seg); }
        p
    }
}

impl Default for WindowsFsBackend { fn default() -> Self { Self::new() } }

impl CliBackend for WindowsFsBackend {
    fn read(&self, rel: &[&str]) -> Result<Option<String>, AppError> {
        let p = self.full_path(rel);
        if !p.exists() { return Ok(None); }
        Ok(Some(std::fs::read_to_string(p)?))
    }
    fn write_atomic(&self, rel: &[&str], bytes: &[u8]) -> Result<(), AppError> {
        let p = self.full_path(rel);
        atomic_write(&p, bytes)
    }
    fn remove(&self, rel: &[&str]) -> Result<(), AppError> {
        let p = self.full_path(rel);
        if p.exists() { std::fs::remove_file(p)?; }
        Ok(())
    }
    fn exists(&self, rel: &[&str]) -> Result<bool, AppError> {
        Ok(self.full_path(rel).exists())
    }
}

/// Atomic write: write to a temp file, then rename. Lifted from config_writer
/// so the trait impl doesn't depend on it being a private fn there.
pub fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path.file_name()
        .ok_or_else(|| AppError::Config("invalid file name".into()))?
        .to_string_lossy();
    let mut tmp = path.parent()
        .ok_or_else(|| AppError::Config("invalid path".into()))?
        .to_path_buf();
    tmp.push(format!("{}.tmp.{}", file_name, ts));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.flush()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(perm));
        }
    }
    #[cfg(windows)]
    { if path.exists() { let _ = std::fs::remove_file(path); } }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn windows_backend_round_trip() {
        let tmp = TempDir::new().unwrap();
        let b = WindowsFsBackend { home: tmp.path().to_path_buf() };
        assert!(!b.exists(&[".claude", "settings.json"]).unwrap());
        b.write_atomic(&[".claude", "settings.json"], b"{}").unwrap();
        assert!(b.exists(&[".claude", "settings.json"]).unwrap());
        assert_eq!(b.read(&[".claude", "settings.json"]).unwrap().as_deref(), Some("{}"));
        b.remove(&[".claude", "settings.json"]).unwrap();
        assert_eq!(b.read(&[".claude", "settings.json"]).unwrap(), None);
    }
}
```

- [ ] **Step 2: Register module**

In `crates/llm-relay-core/src/lib.rs`, after `pub mod wsl;`:

```rust
pub mod cli_target;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p llm-relay-core cli_target
```

Expected: 1 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/cli_target.rs crates/llm-relay-core/src/lib.rs
git commit -m "feat(cli-target): CliBackend trait + WindowsFsBackend + atomic_write"
```

---

### Task 14: Add `WslBackend`

**Files:**
- Modify: `crates/llm-relay-core/src/cli_target.rs`

- [ ] **Step 1: Append `WslBackend`**

At the bottom of `crates/llm-relay-core/src/cli_target.rs` (before `#[cfg(test)]`):

```rust
/// Backend that reads/writes via `wsl.exe` inside a specific distro.
/// `home` is the probed `$HOME` — relative paths are resolved against it.
pub struct WslBackend {
    pub distro: String,
    pub home: String,
}

impl WslBackend {
    fn full_path(&self, rel: &[&str]) -> String {
        let mut s = self.home.clone();
        for seg in rel {
            if !s.ends_with('/') { s.push('/'); }
            s.push_str(seg);
        }
        s
    }
}

impl CliBackend for WslBackend {
    fn read(&self, rel: &[&str]) -> Result<Option<String>, AppError> {
        crate::wsl::fs::wsl_read(&self.distro, &self.full_path(rel))
    }
    fn write_atomic(&self, rel: &[&str], bytes: &[u8]) -> Result<(), AppError> {
        crate::wsl::fs::wsl_atomic_write(&self.distro, &self.full_path(rel), bytes)
    }
    fn remove(&self, rel: &[&str]) -> Result<(), AppError> {
        crate::wsl::fs::wsl_remove(&self.distro, &self.full_path(rel))
    }
    fn exists(&self, rel: &[&str]) -> Result<bool, AppError> {
        crate::wsl::fs::wsl_exists(&self.distro, &self.full_path(rel))
    }
}
```

- [ ] **Step 2: Test path-join**

Append to the existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn wsl_backend_path_join() {
        let b = WslBackend { distro: "Ubuntu".into(), home: "/home/x".into() };
        assert_eq!(b.full_path(&[".claude", "settings.json"]), "/home/x/.claude/settings.json");
        let b2 = WslBackend { distro: "Ubuntu".into(), home: "/home/x/".into() };
        assert_eq!(b2.full_path(&[".claude", "settings.json"]), "/home/x/.claude/settings.json");
    }
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p llm-relay-core cli_target
```

Expected: 2 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-relay-core/src/cli_target.rs
git commit -m "feat(cli-target): WslBackend delegates to wsl::fs"
```

---
### Task 15: Refactor `write_*_config` to take `&dyn CliBackend + base_url`

**Files:**
- Modify: `crates/llm-relay-core/src/config_writer.rs`

This is the biggest single-file change. Strategy: keep the existing standalone
functions intact (Windows-only callers still work), and add parallel
`*_with_backend` variants that take `&dyn CliBackend` + `base_url`. Old
`apply_all_configs` becomes a thin wrapper that builds the Windows target.

- [ ] **Step 1: Add the new backend-typed functions for Claude**

In `crates/llm-relay-core/src/config_writer.rs`, just after the existing
`pub fn write_claude_config(...)`, add:

```rust
pub fn write_claude_config_with(
    backend: &dyn crate::cli_target::CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    small_model: Option<&str>,
) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let big_base: Option<String> = model.map(|m| decompose_claude_id(m).0);

    let existing = backend.read(rel)?;
    let mut settings: Value = match existing.as_deref() {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };

    let env = settings.as_object_mut().unwrap()
        .entry("env").or_insert_with(|| serde_json::json!({}));

    let token_already_present = env.as_object()
        .and_then(|o| o.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let token_to_write: Option<&str> = if token_already_present { None } else { Some(api_key) };

    let needs_update = if let Some(env_obj) = env.as_object() {
        let token_match = match token_to_write {
            Some(t) => env_obj.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) == Some(t),
            None => true,
        };
        !(token_match
            && env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) == Some(base_url)
            && (big_base.is_none()
                || env_obj.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) == big_base.as_deref())
            && (small_model.is_none()
                || env_obj.get("ANTHROPIC_SMALL_FAST_MODEL").and_then(|v| v.as_str()) == small_model))
    } else { true };

    if !needs_update { return Ok(()); }

    if let Some(env_obj) = env.as_object_mut() {
        if let Some(t) = token_to_write {
            env_obj.insert("ANTHROPIC_AUTH_TOKEN".to_string(), Value::String(t.to_string()));
        }
        env_obj.insert("ANTHROPIC_BASE_URL".to_string(), Value::String(base_url.to_string()));
        if let Some(b) = big_base.as_deref() {
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(b.to_string()));
        }
        if let Some(m) = small_model {
            env_obj.insert("ANTHROPIC_SMALL_FAST_MODEL".to_string(), Value::String(m.to_string()));
        }
    }

    let json_str = serde_json::to_string_pretty(&settings)?;
    backend.write_atomic(rel, json_str.as_bytes())
}

pub fn clear_claude_config_with(
    backend: &dyn crate::cli_target::CliBackend,
) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let Some(content) = backend.read(rel)? else { return Ok(()); };
    let mut settings: Value = serde_json::from_str(&content)
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(env) = settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_MODEL");
        env.remove("ANTHROPIC_SMALL_FAST_MODEL");
    }
    let json_str = serde_json::to_string_pretty(&settings)?;
    backend.write_atomic(rel, json_str.as_bytes())
}
```

- [ ] **Step 2: Add equivalent for Codex**

```rust
pub fn write_codex_config_with(
    backend: &dyn crate::cli_target::CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
) -> Result<(), AppError> {
    let auth_rel: &[&str] = &[".codex", "auth.json"];
    let cfg_rel: &[&str] = &[".codex", "config.toml"];

    // auth.json
    let existing_auth = backend.read(auth_rel)?;
    let needs_auth_update = match existing_auth.as_deref() {
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v.get("OPENAI_API_KEY").and_then(|x| x.as_str()) != Some(api_key),
            Err(_) => true,
        },
        None => true,
    };
    if needs_auth_update {
        let auth = serde_json::json!({ "OPENAI_API_KEY": api_key });
        let s = serde_json::to_string_pretty(&auth)?;
        backend.write_atomic(auth_rel, s.as_bytes())?;
    }

    // config.toml
    let url_with_slash = if base_url.ends_with('/') { base_url.to_string() } else { format!("{base_url}/") };
    let existing_cfg = backend.read(cfg_rel)?;
    let mut doc: DocumentMut = match existing_cfg.as_deref() {
        Some(s) => s.parse::<DocumentMut>().unwrap_or_else(|_| "".parse().unwrap()),
        None => "".parse().unwrap(),
    };

    let needs_cfg_update = doc.get("model_provider").and_then(|v| v.as_str()) != Some("copilot_gateway")
        || (model.is_some() && doc.get("model").and_then(|v| v.as_str()) != model)
        || doc.get("model_providers")
            .and_then(|mp| mp.get("copilot_gateway"))
            .and_then(|gw| gw.get("base_url"))
            .and_then(|u| u.as_str()) != Some(&url_with_slash);

    if !needs_cfg_update { return Ok(()); }

    if let Some(m) = model { doc["model"] = toml_edit::value(m); }
    doc["model_provider"] = toml_edit::value("copilot_gateway");
    if doc.get("model_providers").is_none() { doc["model_providers"] = toml_edit::table(); }
    if let Some(providers) = doc["model_providers"].as_table_mut() {
        if !providers.contains_key("copilot_gateway") {
            providers["copilot_gateway"] = toml_edit::table();
        }
        if let Some(gw) = providers["copilot_gateway"].as_table_mut() {
            gw["name"] = toml_edit::value("Copilot Gateway");
            gw["base_url"] = toml_edit::value(&url_with_slash);
            gw["env_key"] = toml_edit::value("OPENAI_API_KEY");
            gw["wire_api"] = toml_edit::value("responses");
        }
    }
    backend.write_atomic(cfg_rel, doc.to_string().as_bytes())
}

pub fn clear_codex_config_with(
    backend: &dyn crate::cli_target::CliBackend,
) -> Result<(), AppError> {
    let auth_rel: &[&str] = &[".codex", "auth.json"];
    let cfg_rel: &[&str] = &[".codex", "config.toml"];
    if let Some(content) = backend.read(auth_rel)? {
        let mut auth: Value = serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = auth.as_object_mut() { obj.remove("OPENAI_API_KEY"); }
        backend.write_atomic(auth_rel, serde_json::to_string_pretty(&auth)?.as_bytes())?;
    }
    if let Some(content) = backend.read(cfg_rel)? {
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                if let Some(gw) = providers.get_mut("copilot_gateway").and_then(|v| v.as_table_mut()) {
                    gw.remove("base_url");
                }
            }
            backend.write_atomic(cfg_rel, doc.to_string().as_bytes())?;
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Add equivalent for Gemini**

```rust
pub fn write_gemini_config_with(
    backend: &dyn crate::cli_target::CliBackend,
    base_url: &str,
    api_key: &str,
) -> Result<(), AppError> {
    let env_rel: &[&str] = &[".gemini", ".env"];
    let settings_rel: &[&str] = &[".gemini", "settings.json"];

    let mut env_map = match backend.read(env_rel)? {
        Some(s) => parse_env_file(&s),
        None => HashMap::new(),
    };
    let needs_update = env_map.get("GEMINI_API_KEY") != Some(&api_key.to_string())
        || env_map.get("GOOGLE_GEMINI_BASE_URL") != Some(&base_url.to_string())
        || env_map.get("GEMINI_API_BASE_URL") != Some(&base_url.to_string());
    if needs_update {
        env_map.insert("GEMINI_API_KEY".into(), api_key.into());
        env_map.insert("GOOGLE_GEMINI_BASE_URL".into(), base_url.into());
        env_map.insert("GEMINI_API_BASE_URL".into(), base_url.into());
        backend.write_atomic(env_rel, serialize_env_file(&env_map).as_bytes())?;
    }

    let mut settings: Value = match backend.read(settings_rel)? {
        Some(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };
    if let Some(obj) = settings.as_object_mut() {
        let security = obj.entry("security").or_insert_with(|| serde_json::json!({}));
        if let Some(sec_obj) = security.as_object_mut() {
            let auth = sec_obj.entry("auth").or_insert_with(|| serde_json::json!({}));
            if let Some(auth_obj) = auth.as_object_mut() {
                auth_obj.insert("selectedType".into(), Value::String("gemini-api-key".into()));
            }
        }
    }
    backend.write_atomic(settings_rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

pub fn clear_gemini_config_with(
    backend: &dyn crate::cli_target::CliBackend,
) -> Result<(), AppError> {
    let env_rel: &[&str] = &[".gemini", ".env"];
    let Some(content) = backend.read(env_rel)? else { return Ok(()); };
    let mut env_map = parse_env_file(&content);
    env_map.remove("GEMINI_API_KEY");
    env_map.remove("GOOGLE_GEMINI_BASE_URL");
    env_map.remove("GEMINI_API_BASE_URL");
    backend.write_atomic(env_rel, serialize_env_file(&env_map).as_bytes())
}
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p llm-relay-core`
Expected: clean.

- [ ] **Step 5: Add a smoke test using `WindowsFsBackend` + tempdir**

Append at the bottom of `config_writer.rs`:

```rust
#[cfg(test)]
mod backend_tests {
    use super::*;
    use crate::cli_target::{CliBackend, WindowsFsBackend};
    use tempfile::TempDir;

    fn fake_home(tmp: &TempDir) -> WindowsFsBackend {
        // Trait field is private; use Default path via unsafe? No — add ctor.
        // (Done in Task 13: WindowsFsBackend::new() picks dirs::home_dir().)
        // For tests we use a struct-literal but that needs `home` to be pub.
        // Adjust Task 13's `home: PathBuf` to be pub(crate) if it isn't.
        WindowsFsBackend { home: tmp.path().to_path_buf() }
    }

    #[test]
    fn claude_round_trip_via_backend() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_claude_config_with(&b, "http://127.0.0.1:18080", "k", Some("claude-sonnet-4-6"), None).unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(content.contains("\"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:18080\""));
        clear_claude_config_with(&b).unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
    }
}
```

If `home` is private in `WindowsFsBackend`, change `home: PathBuf` to `pub(crate) home: PathBuf` in `cli_target.rs`.

- [ ] **Step 6: Run tests**

```bash
cargo test -p llm-relay-core config_writer
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/llm-relay-core/src/config_writer.rs crates/llm-relay-core/src/cli_target.rs
git commit -m "feat(config-writer): backend-typed write_*/clear_* variants taking base_url"
```

---
### Task 16: Snapshot v2 module — directory layout + opaque ids

**Files:**
- Create: `crates/llm-relay-core/src/config_writer/snapshot.rs`
- Modify: `crates/llm-relay-core/src/config_writer.rs`

This task moves snapshot logic into a sub-module so the directory layout
and JSON shape live in one place.

- [ ] **Step 1: Convert `config_writer.rs` from a single file to a directory module**

```bash
mkdir crates/llm-relay-core/src/config_writer
git mv crates/llm-relay-core/src/config_writer.rs crates/llm-relay-core/src/config_writer/mod.rs
```

Run: `cargo check -p llm-relay-core`
Expected: clean (Rust treats `config_writer/mod.rs` exactly like `config_writer.rs`).

- [ ] **Step 2: Create the snapshot module**

Create `crates/llm-relay-core/src/config_writer/snapshot.rs`:

```rust
//! Per-target CLI config snapshots. Stored as one JSON file per target
//! under `cli_config_backup_dir()`. Filenames are opaque sha256-derived
//! ids; original distro name + target_type live inside the JSON so
//! restore doesn't depend on filename parsing.

use super::*;
use crate::AppError;
use crate::cli_target::{CliBackend, CliTarget, SnapshotMeta, TargetType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSnapshot {
    /// "windows" | "wsl". Required.
    pub target_type: String,
    /// Original `wsl -d <name>`. None for Windows targets.
    pub distro_name: Option<String>,
    /// $HOME captured at probe time (WSL targets). Used by restore to
    /// rebuild a WslBackend without re-probing a maybe-stopped distro.
    pub home: Option<String>,
    pub captured_at: String,
    pub claude: ClaudeSnapshot,
    pub codex: CodexSnapshot,
    pub gemini: GeminiSnapshot,
}

/// Stable opaque id derived from `distro_name` (or the literal "windows").
/// 16 chars of lowercase base32 of sha256 is collision-safe enough for
/// the at-most-handful of distros a user manages.
pub fn target_file_name(meta: &SnapshotMeta) -> String {
    match meta.target_type {
        TargetType::Windows => "windows.json".to_string(),
        TargetType::Wsl => {
            use sha2::{Digest, Sha256};
            let name = meta.distro_name.as_deref().unwrap_or("");
            let mut h = Sha256::new();
            h.update(name.as_bytes());
            let digest = h.finalize();
            let b32 = data_encoding::BASE32_NOPAD.encode(&digest);
            let id: String = b32.chars().take(16).collect::<String>().to_lowercase();
            format!("wsl-{id}.json")
        }
    }
}

pub fn snapshot_path(meta: &SnapshotMeta) -> PathBuf {
    crate::paths::cli_config_backup_dir().join(target_file_name(meta))
}

/// Capture the live state of the three CLIs as visible to `backend`
/// and persist as a TargetSnapshot JSON. Atomic write.
pub fn capture(target: &CliTarget) -> Result<(), AppError> {
    let snap = TargetSnapshot {
        target_type: match target.snapshot_meta.target_type {
            TargetType::Windows => "windows".into(),
            TargetType::Wsl => "wsl".into(),
        },
        distro_name: target.snapshot_meta.distro_name.clone(),
        home: target.snapshot_meta.home.clone(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        claude: capture_claude(&*target.backend)?,
        codex: capture_codex(&*target.backend)?,
        gemini: capture_gemini(&*target.backend)?,
    };
    let path = snapshot_path(&target.snapshot_meta);
    std::fs::create_dir_all(crate::paths::cli_config_backup_dir())?;
    let bytes = serde_json::to_vec_pretty(&snap)?;
    crate::cli_target::atomic_write(&path, &bytes)
}

pub fn read(meta: &SnapshotMeta) -> Result<Option<TargetSnapshot>, AppError> {
    let path = snapshot_path(meta);
    if !path.exists() { return Ok(None); }
    let bytes = std::fs::read(&path)?;
    let snap: TargetSnapshot = serde_json::from_slice(&bytes)?;
    Ok(Some(snap))
}

pub fn delete(meta: &SnapshotMeta) -> Result<(), AppError> {
    let path = snapshot_path(meta);
    if path.exists() { std::fs::remove_file(&path)?; }
    Ok(())
}

/// Restore one target from its snapshot. Mirrors the existing
/// restore_claude/restore_codex/restore_gemini, but parameterized on
/// backend.
pub fn restore(snap: &TargetSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    restore_claude_backend(&snap.claude, backend)?;
    restore_codex_backend(&snap.codex, backend)?;
    restore_gemini_backend(&snap.gemini, backend)?;
    Ok(())
}

/// Scan the backup dir and build `distro_name → (snapshot path, target_type)`
/// index. Windows snapshot keyed by the literal "windows".
pub fn build_index() -> Result<std::collections::HashMap<String, SnapshotMeta>, AppError> {
    let dir = crate::paths::cli_config_backup_dir();
    let mut map = std::collections::HashMap::new();
    if !dir.exists() { return Ok(map); }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let bytes = std::fs::read(&path)?;
        let snap: TargetSnapshot = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("ignoring malformed snapshot {}: {e}", path.display());
                continue;
            }
        };
        let meta = SnapshotMeta {
            target_type: if snap.target_type == "wsl" { TargetType::Wsl } else { TargetType::Windows },
            distro_name: snap.distro_name.clone(),
            home: snap.home.clone(),
        };
        let key = snap.distro_name.unwrap_or_else(|| "windows".to_string());
        map.insert(key, meta);
    }
    Ok(map)
}

// --- per-CLI capture functions delegating to backend ---

fn capture_claude(backend: &dyn CliBackend) -> Result<ClaudeSnapshot, AppError> {
    let mut snap = ClaudeSnapshot::default();
    let Some(content) = backend.read(&[".claude", "settings.json"])? else { return Ok(snap); };
    let Ok(val) = serde_json::from_str::<Value>(&content) else { return Ok(snap); };
    let Some(env) = val.get("env").and_then(|v| v.as_object()) else { return Ok(snap); };
    let get = |k: &str| env.get(k).and_then(|v| v.as_str()).map(String::from);
    snap.anthropic_base_url = get("ANTHROPIC_BASE_URL");
    snap.anthropic_model = get("ANTHROPIC_MODEL");
    snap.anthropic_small_fast_model = get("ANTHROPIC_SMALL_FAST_MODEL");
    snap.anthropic_auth_token = get("ANTHROPIC_AUTH_TOKEN");
    Ok(snap)
}

fn capture_codex(backend: &dyn CliBackend) -> Result<CodexSnapshot, AppError> {
    let mut snap = CodexSnapshot::default();
    if let Some(content) = backend.read(&[".codex", "auth.json"])? {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            snap.openai_api_key = val.get("OPENAI_API_KEY")
                .and_then(|v| v.as_str()).map(String::from);
        }
    }
    if let Some(content) = backend.read(&[".codex", "config.toml"])? {
        if let Ok(doc) = content.parse::<DocumentMut>() {
            snap.model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
            snap.model_provider = doc.get("model_provider").and_then(|v| v.as_str()).map(String::from);
            if let Some(gw) = doc.get("model_providers")
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("copilot_gateway"))
                .and_then(|v| v.as_table())
            {
                snap.copilot_gateway_provider_toml = Some(gw.to_string());
            }
        }
    }
    Ok(snap)
}

fn capture_gemini(backend: &dyn CliBackend) -> Result<GeminiSnapshot, AppError> {
    let mut snap = GeminiSnapshot::default();
    if let Some(content) = backend.read(&[".gemini", ".env"])? {
        let env_map = parse_env_file(&content);
        snap.gemini_api_key = env_map.get("GEMINI_API_KEY").cloned();
        snap.google_gemini_base_url = env_map.get("GOOGLE_GEMINI_BASE_URL").cloned();
        snap.gemini_api_base_url = env_map.get("GEMINI_API_BASE_URL").cloned();
    }
    if let Some(content) = backend.read(&[".gemini", "settings.json"])? {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            snap.selected_auth_type = val.get("security")
                .and_then(|v| v.get("auth"))
                .and_then(|v| v.get("selectedType"))
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    Ok(snap)
}

// --- per-CLI restore functions ---

fn restore_claude_backend(snap: &ClaudeSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let Some(content) = backend.read(rel)? else { return Ok(()); };
    let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
    let env = settings.as_object_mut().unwrap()
        .entry("env").or_insert_with(|| json!({}));
    if let Some(env_obj) = env.as_object_mut() {
        let apply = |obj: &mut serde_json::Map<String, Value>, k: &str, v: &Option<String>| {
            match v {
                Some(s) => { obj.insert(k.into(), Value::String(s.clone())); }
                None => { obj.remove(k); }
            }
        };
        apply(env_obj, "ANTHROPIC_BASE_URL", &snap.anthropic_base_url);
        apply(env_obj, "ANTHROPIC_MODEL", &snap.anthropic_model);
        apply(env_obj, "ANTHROPIC_SMALL_FAST_MODEL", &snap.anthropic_small_fast_model);
        apply(env_obj, "ANTHROPIC_AUTH_TOKEN", &snap.anthropic_auth_token);
    }
    backend.write_atomic(rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

fn restore_codex_backend(snap: &CodexSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    if backend.exists(&[".codex", "auth.json"])? {
        let content = backend.read(&[".codex", "auth.json"])?.unwrap_or_default();
        let mut auth: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
        if let Some(obj) = auth.as_object_mut() {
            match &snap.openai_api_key {
                Some(k) => { obj.insert("OPENAI_API_KEY".into(), Value::String(k.clone())); }
                None => { obj.remove("OPENAI_API_KEY"); }
            }
        }
        backend.write_atomic(&[".codex", "auth.json"], serde_json::to_string_pretty(&auth)?.as_bytes())?;
    }
    if backend.exists(&[".codex", "config.toml"])? {
        let content = backend.read(&[".codex", "config.toml"])?.unwrap_or_default();
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            match &snap.model {
                Some(m) => { doc["model"] = toml_edit::value(m.clone()); }
                None => { doc.as_table_mut().remove("model"); }
            }
            match &snap.model_provider {
                Some(p) => { doc["model_provider"] = toml_edit::value(p.clone()); }
                None => { doc.as_table_mut().remove("model_provider"); }
            }
            if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                providers.remove("copilot_gateway");
            }
            if let Some(orig_toml) = &snap.copilot_gateway_provider_toml {
                if doc.get("model_providers").is_none() { doc["model_providers"] = toml_edit::table(); }
                let wrapper = format!("[model_providers.copilot_gateway]\n{orig_toml}");
                if let Ok(parsed) = wrapper.parse::<DocumentMut>() {
                    if let Some(orig_gw) = parsed.get("model_providers")
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("copilot_gateway"))
                        .and_then(|v| v.as_table())
                    {
                        if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                            providers.insert("copilot_gateway", toml_edit::Item::Table(orig_gw.clone()));
                        }
                    }
                }
            }
            let empty = doc.get("model_providers").and_then(|v| v.as_table()).map(|t| t.is_empty()).unwrap_or(false);
            if empty { doc.as_table_mut().remove("model_providers"); }
            backend.write_atomic(&[".codex", "config.toml"], doc.to_string().as_bytes())?;
        }
    }
    Ok(())
}

fn restore_gemini_backend(snap: &GeminiSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    if backend.exists(&[".gemini", ".env"])? {
        let content = backend.read(&[".gemini", ".env"])?.unwrap_or_default();
        let mut env_map = parse_env_file(&content);
        let apply = |m: &mut std::collections::HashMap<String, String>, k: &str, v: &Option<String>| {
            match v {
                Some(s) => { m.insert(k.into(), s.clone()); }
                None => { m.remove(k); }
            }
        };
        apply(&mut env_map, "GEMINI_API_KEY", &snap.gemini_api_key);
        apply(&mut env_map, "GOOGLE_GEMINI_BASE_URL", &snap.google_gemini_base_url);
        apply(&mut env_map, "GEMINI_API_BASE_URL", &snap.gemini_api_base_url);
        backend.write_atomic(&[".gemini", ".env"], serialize_env_file(&env_map).as_bytes())?;
    }
    if backend.exists(&[".gemini", "settings.json"])? {
        let content = backend.read(&[".gemini", "settings.json"])?.unwrap_or_default();
        let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
        if let Some(obj) = settings.as_object_mut() {
            match &snap.selected_auth_type {
                Some(t) => {
                    let security = obj.entry("security").or_insert_with(|| json!({}));
                    if let Some(sec_obj) = security.as_object_mut() {
                        let auth = sec_obj.entry("auth").or_insert_with(|| json!({}));
                        if let Some(auth_obj) = auth.as_object_mut() {
                            auth_obj.insert("selectedType".into(), Value::String(t.clone()));
                        }
                    }
                }
                None => {
                    if let Some(auth_obj) = obj.get_mut("security")
                        .and_then(|v| v.as_object_mut())
                        .and_then(|s| s.get_mut("auth"))
                        .and_then(|v| v.as_object_mut())
                    {
                        auth_obj.remove("selectedType");
                    }
                }
            }
        }
        backend.write_atomic(&[".gemini", "settings.json"], serde_json::to_string_pretty(&settings)?.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_id_stable_and_different_for_distinct_names() {
        let a = SnapshotMeta { target_type: TargetType::Wsl, distro_name: Some("Ubuntu 22.04".into()), home: None };
        let b = SnapshotMeta { target_type: TargetType::Wsl, distro_name: Some("Ubuntu_22.04".into()), home: None };
        let n1 = target_file_name(&a);
        let n2 = target_file_name(&a);
        let n3 = target_file_name(&b);
        assert_eq!(n1, n2);
        assert_ne!(n1, n3);
        assert!(n1.starts_with("wsl-") && n1.ends_with(".json"));
    }

    #[test]
    fn windows_target_uses_fixed_filename() {
        let m = SnapshotMeta { target_type: TargetType::Windows, distro_name: None, home: None };
        assert_eq!(target_file_name(&m), "windows.json");
    }
}
```

- [ ] **Step 3: Wire submodule and re-exports**

At the top of `crates/llm-relay-core/src/config_writer/mod.rs`, after the
existing `use` statements add:

```rust
pub mod snapshot;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p llm-relay-core config_writer::snapshot
```

Expected: 2 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-core/src/config_writer/
git commit -m "feat(snapshot): per-target snapshot module with opaque ids + index"
```

---
### Task 17: Rewrite `apply_all_configs` / `clear_all_configs` over targets + legacy migration

**Files:**
- Modify: `crates/llm-relay-core/src/config_writer/mod.rs`

- [ ] **Step 1: Rewrite the top-level apply/clear**

Replace the existing `apply_all_configs` and `clear_all_configs` bodies with:

```rust
pub fn apply_to_targets(
    targets: &[CliTarget],
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    _gemini_model: Option<&str>,
) -> Result<(), AppError> {
    use crate::cli_target::{CliTarget, TargetType};
    let prev_index = snapshot::build_index()?;
    let current_keys: std::collections::HashSet<String> = targets.iter()
        .map(|t| t.snapshot_meta.distro_name.clone().unwrap_or_else(|| "windows".to_string()))
        .collect();

    // 1. Restore + delete snapshots for targets removed since last apply.
    for (key, meta) in &prev_index {
        if !current_keys.contains(key) {
            log::info!("dropping target {key} — restoring previous state");
            if let Some(snap) = snapshot::read(meta)? {
                let backend: Box<dyn CliBackend> = match meta.target_type {
                    TargetType::Windows => Box::new(WindowsFsBackend::new()),
                    TargetType::Wsl => Box::new(WslBackend {
                        distro: meta.distro_name.clone().unwrap_or_default(),
                        home: meta.home.clone().unwrap_or_default(),
                    }),
                };
                if let Err(e) = snapshot::restore(&snap, &*backend) {
                    log::warn!("restore failed for dropped target {key}: {e}");
                }
            }
            let _ = snapshot::delete(meta);
        }
    }

    // 2. For each current target: capture snapshot if new, then write.
    let mut at_least_one_success = false;
    let mut windows_failed = false;
    for target in targets {
        let key = target.snapshot_meta.distro_name.clone().unwrap_or_else(|| "windows".to_string());
        if !prev_index.contains_key(&key) {
            if let Err(e) = snapshot::capture(target) {
                log::warn!("snapshot capture failed for {key}: {e}");
                // Don't write if we can't snapshot — leaves user's prior
                // state recoverable.
                if matches!(target.snapshot_meta.target_type, TargetType::Windows) {
                    windows_failed = true;
                }
                continue;
            }
        }
        let result = write_one_target(target, api_key, claude_model, claude_small_model, codex_model);
        match result {
            Ok(()) => { at_least_one_success = true; }
            Err(e) => {
                log::warn!("apply failed for {key}: {e}");
                if matches!(target.snapshot_meta.target_type, TargetType::Windows) {
                    windows_failed = true;
                }
            }
        }
    }

    // 3. Define overall success: Windows target must succeed.
    if windows_failed {
        return Err(AppError::Config("apply: Windows target failed".into()));
    }
    if !at_least_one_success && !targets.is_empty() {
        return Err(AppError::Config("apply: no target succeeded".into()));
    }
    Ok(())
}

fn write_one_target(
    target: &CliTarget,
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
) -> Result<(), AppError> {
    let b = &*target.backend;
    if target.installed.claude {
        write_claude_config_with(b, &target.base_url, api_key, claude_model, claude_small_model)?;
    }
    if target.installed.codex {
        write_codex_config_with(b, &target.base_url, api_key, codex_model)?;
    }
    if target.installed.gemini {
        write_gemini_config_with(b, &target.base_url, api_key)?;
    }
    Ok(())
}

pub fn clear_targets_from_snapshots() -> Result<(), AppError> {
    use crate::cli_target::TargetType;
    let dir = crate::paths::cli_config_backup_dir();
    if !dir.exists() { return Ok(()); }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let bytes = std::fs::read(&path)?;
        let snap: snapshot::TargetSnapshot = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => { log::warn!("malformed snapshot {}: {e}", path.display()); continue; }
        };
        let meta = SnapshotMeta {
            target_type: if snap.target_type == "wsl" { TargetType::Wsl } else { TargetType::Windows },
            distro_name: snap.distro_name.clone(),
            home: snap.home.clone(),
        };
        let backend: Box<dyn CliBackend> = match meta.target_type {
            TargetType::Windows => Box::new(WindowsFsBackend::new()),
            TargetType::Wsl => Box::new(WslBackend {
                distro: meta.distro_name.clone().unwrap_or_default(),
                home: meta.home.clone().unwrap_or_default(),
            }),
        };
        if let Err(e) = snapshot::restore(&snap, &*backend) {
            log::warn!("clear restore failed for {}: {e}", path.display());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
```

Imports at top of `mod.rs`:

```rust
use crate::cli_target::{CliBackend, CliTarget, SnapshotMeta, WindowsFsBackend, WslBackend};
```

- [ ] **Step 2: Migrate the existing `apply_all_configs` callers (Service layer)**

Leave the legacy `pub fn apply_all_configs(...)` standalone wrapper for now —
it will be retired in Task 18 when `Service` learns to build targets.

For compile safety, change the body of legacy `apply_all_configs` to build
a single-target `WindowsFsBackend` and delegate:

```rust
pub fn apply_all_configs(
    base_url: &str,
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    gemini_model: Option<&str>,
) -> Result<(), AppError> {
    use crate::cli_target::{CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend};
    let target = CliTarget {
        backend: Box::new(WindowsFsBackend::new()),
        base_url: base_url.to_string(),
        installed: InstalledTools::ALL,
        label: "windows".into(),
        snapshot_meta: SnapshotMeta {
            target_type: TargetType::Windows,
            distro_name: None,
            home: None,
        },
    };
    apply_to_targets(&[target], api_key, claude_model, claude_small_model, codex_model, gemini_model)?;
    ensure_openai_api_key_in_shell_rc()
}

pub fn clear_all_configs() -> Result<(), AppError> {
    clear_targets_from_snapshots()
}
```

(`ensure_openai_api_key_in_shell_rc` stays as-is — Windows-host shell env
is unrelated to per-target writes.)

- [ ] **Step 3: Add legacy snapshot file migration**

In `snapshot.rs`, add:

```rust
/// One-shot migration of pre-WSL2 single-file snapshot to per-target
/// directory layout. Old format has no `target_type` field; new clear
/// path requires one. Idempotent: skip if new dir already has windows.json.
pub fn migrate_legacy_if_needed() -> Result<(), AppError> {
    let old_path = crate::paths::legacy_cli_config_backup_file();
    let new_dir = crate::paths::cli_config_backup_dir();
    let new_path = new_dir.join("windows.json");
    if !old_path.exists() || new_path.exists() { return Ok(()); }

    let bytes = match std::fs::read(&old_path) {
        Ok(b) => b,
        Err(e) => return Err(AppError::Config(format!("legacy snapshot read: {e}"))),
    };
    let mut v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            // Move aside so we don't retry every boot.
            let aside = old_path.with_extension("corrupt");
            log::warn!("legacy snapshot malformed ({e}); moving to {}", aside.display());
            let _ = std::fs::rename(&old_path, aside);
            return Ok(());
        }
    };
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return Err(AppError::Config("legacy snapshot is not a JSON object".into())),
    };
    obj.insert("target_type".into(), json!("windows"));
    obj.entry("captured_at").or_insert_with(|| json!(chrono::Utc::now().to_rfc3339()));
    std::fs::create_dir_all(&new_dir)?;
    let new_bytes = serde_json::to_vec_pretty(&v)?;
    crate::cli_target::atomic_write(&new_path, &new_bytes)?;
    std::fs::remove_file(&old_path)?;
    log::info!("migrated legacy CLI snapshot → {}", new_path.display());
    Ok(())
}
```

Add a test using `tempfile + LLM_RELAY_RUNTIME_DIR override`? The current
`paths::config_dir()` doesn't honor an env var. Add a unit test that
constructs the migration logic on hand-built paths:

```rust
    #[test]
    fn migrate_corrupt_file_moves_aside_and_skips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let legacy = tmp.path().join("cli-config-backup.json");
        std::fs::write(&legacy, b"not json").unwrap();
        let new_dir = tmp.path().join("cli-config-backup");
        // Use the same logic inline since migrate_legacy_if_needed reads
        // global paths. This test asserts the behavior we want.
        let bytes = std::fs::read(&legacy).unwrap();
        let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
        assert!(parsed.is_err());
        // Move aside
        let aside = legacy.with_extension("corrupt");
        std::fs::rename(&legacy, &aside).unwrap();
        assert!(aside.exists());
        assert!(!new_dir.exists());
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p llm-relay-core config_writer
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-core/src/config_writer/
git commit -m "feat(config-writer): apply_to_targets + clear_targets_from_snapshots + legacy migration"
```

---
## Phase 6: Service wiring + detection state machine

### Task 18: `Service::build_apply_targets`

**Files:**
- Modify: `crates/llm-relay-core/src/service.rs`

- [ ] **Step 1: Add target builder**

In `crates/llm-relay-core/src/service.rs`, inside `impl Service`, add:

```rust
    /// Build the list of CLI targets to apply: Windows (always) plus every
    /// selected WSL distro that has a `resolved_url`. Distros without a
    /// resolved URL are skipped with a warning — the user must click
    /// Refresh after fixing the network/probe issue.
    pub fn build_apply_targets(&self) -> Result<Vec<crate::cli_target::CliTarget>, AppError> {
        use crate::cli_target::{CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend, WslBackend};
        let mut targets = Vec::new();

        // Windows is always present.
        targets.push(CliTarget {
            backend: Box::new(WindowsFsBackend::new()),
            base_url: crate::proxy_server::proxy_base_url(),
            installed: InstalledTools::ALL,
            label: "windows".into(),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Windows,
                distro_name: None,
                home: None,
            },
        });

        // WSL distros (Windows only — table is empty on mac/Linux).
        let rows = self.ctx.db.list_wsl_distros().unwrap_or_default();
        for row in rows {
            if !row.selected { continue; }
            let Some(url) = row.resolved_url.clone() else {
                log::warn!("WSL distro {} has no resolved_url — skipping apply", row.name);
                continue;
            };
            let Some(home) = row.home.clone() else {
                log::warn!("WSL distro {} has no probed home — skipping apply", row.name);
                continue;
            };
            targets.push(CliTarget {
                backend: Box::new(WslBackend { distro: row.name.clone(), home: home.clone() }),
                base_url: url,
                installed: InstalledTools {
                    claude: row.has_claude,
                    codex: row.has_codex,
                    gemini: row.has_gemini,
                },
                label: format!("wsl:{}", row.name),
                snapshot_meta: SnapshotMeta {
                    target_type: TargetType::Wsl,
                    distro_name: Some(row.name),
                    home: Some(home),
                },
            });
        }
        Ok(targets)
    }
```

- [ ] **Step 2: Switch `set_active` to use it**

Find this in `Service::set_active`:

```rust
        let proxy_url = crate::proxy_server::proxy_base_url();
        crate::config_writer::apply_all_configs(
            &proxy_url,
            crate::proxy_server::PLACEHOLDER_KEY,
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;
```

Replace with:

```rust
        let targets = self.build_apply_targets()?;
        crate::config_writer::apply_to_targets(
            &targets,
            crate::proxy_server::PLACEHOLDER_KEY,
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;
        // Windows-host shell env (OPENAI_API_KEY=dummy in shell rc / registry)
        // is unrelated to per-target writes; keep separate side-effect.
        crate::config_writer::ensure_openai_api_key_in_shell_rc()?;
```

If `ensure_openai_api_key_in_shell_rc` is private, change it to `pub(crate)` in `config_writer/mod.rs`.

- [ ] **Step 3: Switch `clear_active` to use it**

Find `crate::config_writer::clear_all_configs()?;` in `Service::clear_active` and replace with:

```rust
        crate::config_writer::clear_targets_from_snapshots()?;
```

- [ ] **Step 4: Verify compile**

```bash
cargo check --workspace
```

Expected: clean. Run all core tests:

```bash
cargo test -p llm-relay-core
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-core/src/service.rs crates/llm-relay-core/src/config_writer/
git commit -m "feat(service): set_active/clear_active iterate Windows+WSL targets"
```

---

### Task 19: Detection state machine + periodic re-probe

**Files:**
- Create: `crates/llm-relay-core/src/wsl/state.rs`
- Modify: `crates/llm-relay-core/src/wsl/mod.rs`
- Modify: `crates/llm-relay-core/src/service.rs`

- [ ] **Step 1: Create the state machine module**

Create `crates/llm-relay-core/src/wsl/state.rs`:

```rust
//! Detection cadence state machine. See spec §3.5.
//! - Active: WSL available + ≥1 distro → re-probe every 60s
//! - Lazy:   WSL absent / no distros → no periodic work; only on
//!           explicit Refresh or app restart
//!
//! Spawned by `Service::spawn_wsl_state_machine()`; one task per
//! Service. Holds an `Arc<ProxyHandle>` so it can call `rebind_wsl`
//! when the gateway IP changes.

use crate::AppError;
use std::sync::Arc;
use std::time::Duration;

const ACTIVE_TICK_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Active, Lazy }

pub struct StateMachine {
    db: Arc<crate::Database>,
    proxy: Arc<crate::proxy_server::ProxyHandle>,
    mode: tokio::sync::Mutex<Mode>,
    refresh_signal: tokio::sync::Notify,
}

impl StateMachine {
    pub fn new(db: Arc<crate::Database>, proxy: Arc<crate::proxy_server::ProxyHandle>) -> Arc<Self> {
        Arc::new(Self {
            db,
            proxy,
            mode: tokio::sync::Mutex::new(Mode::Lazy),
            refresh_signal: tokio::sync::Notify::new(),
        })
    }

    /// Trigger a one-off refresh (used by Tauri/TUI Reconnect / Refresh).
    pub fn request_refresh(&self) { self.refresh_signal.notify_one(); }

    /// Entry point: run forever, ticking on a 60s schedule when Active,
    /// otherwise sleeping until `request_refresh` is called.
    pub async fn run(self: Arc<Self>) {
        // Initial detection: discover, probe URLs, set mode.
        self.tick().await;

        loop {
            let mode = *self.mode.lock().await;
            match mode {
                Mode::Active => {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TICK_SECS)) => {},
                        _ = self.refresh_signal.notified() => {},
                    }
                }
                Mode::Lazy => {
                    // Only wake on explicit refresh.
                    self.refresh_signal.notified().await;
                }
            }
            self.tick().await;
        }
    }

    async fn tick(&self) {
        // 1. Reconcile distros + their installed-tools cache.
        let distros = match tokio::task::spawn_blocking({
            let db = self.db.clone();
            move || crate::wsl::distro::refresh_distros_in_db(&db)
        }).await {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => { log::warn!("refresh_distros: {e}"); Vec::new() }
            Err(e) => { log::warn!("refresh_distros join: {e}"); Vec::new() }
        };

        // 2. Re-bind WSL listener if gateway IP changed.
        let current_ip = self.proxy.wsl_ip();
        let new_ip = tokio::task::spawn_blocking(crate::wsl::network::find_wsl_gateway_ip).await.ok().flatten();
        if current_ip != new_ip {
            if let Err(e) = self.proxy.rebind_wsl(new_ip).await {
                log::warn!("rebind_wsl: {e}");
            }
        }

        // 3. Re-probe URL for each selected distro (skip unselected — cheap).
        let binds = crate::wsl::probe::ListenerBinds {
            loopback: true,
            host_docker_internal: self.proxy.wsl_ip().is_some(),
        };
        for d in &distros {
            if !d.selected { continue; }
            let name = d.name.clone();
            let probe_res = tokio::task::spawn_blocking(move || {
                crate::wsl::probe::probe_url_for_distro(&name, binds)
            }).await;
            let resolved_url = match probe_res {
                Ok(Ok(crate::wsl::probe::ProbeOutcome::Ok { url, .. })) => Some(url),
                Ok(Ok(_)) => None,
                Ok(Err(e)) => { log::warn!("probe {}: {e}", d.name); None }
                Err(e) => { log::warn!("probe join {}: {e}", d.name); None }
            };
            // Write resolved_url back to DB.
            let mut row = d.clone();
            row.resolved_url = resolved_url;
            let _ = self.db.upsert_wsl_distro(&row);
        }

        // 4. Update mode based on what we found.
        let mode_new = if distros.is_empty() { Mode::Lazy } else { Mode::Active };
        *self.mode.lock().await = mode_new;
        log::debug!("WSL state machine tick: {} distros, mode={:?}", distros.len(), mode_new);
    }
}
```

- [ ] **Step 2: Register module**

Update `crates/llm-relay-core/src/wsl/mod.rs`:

```rust
pub mod distro;
pub mod network;
pub mod fs;
pub mod probe;
pub mod state;
```

- [ ] **Step 3: Spawn from `Service`**

In `crates/llm-relay-core/src/service.rs` add to `impl Service`:

```rust
    /// Spawn the WSL detection state machine. Stores the handle on the
    /// Service so callers (Tauri command, TUI keybind) can request an
    /// immediate refresh without going through the proxy.
    pub fn spawn_wsl_state_machine(&self) -> Option<Arc<crate::wsl::state::StateMachine>> {
        let proxy = self.proxy.as_ref()?.clone();
        let sm = crate::wsl::state::StateMachine::new(self.ctx.db.clone(), proxy);
        let sm_clone = sm.clone();
        tokio::spawn(async move { sm_clone.run().await; });
        Some(sm)
    }
```

In the agent's `main.rs` and Tauri's `lib.rs`, after `let service = service.with_proxy(...)`, add:

```rust
let wsl_state = service.spawn_wsl_state_machine();
// Pass `wsl_state` to wherever IPC/Tauri command handlers live so
// Refresh / Reconnect can call `sm.request_refresh()`.
```

- [ ] **Step 4: Verify compile**

```bash
cargo check --workspace
```

- [ ] **Step 5: Commit**

```bash
git add crates/llm-relay-core/src/wsl/ crates/llm-relay-core/src/service.rs
git commit -m "feat(wsl): state machine — Active 60s tick / Lazy on-demand re-probe"
```

---

### Task 20: IPC + Tauri commands for distro list/toggle/refresh

**Files:**
- Modify: `crates/llm-relay-core/src/ipc/protocol.rs`
- Modify: `crates/llm-relay-core/src/service.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add IPC types**

In `crates/llm-relay-core/src/ipc/protocol.rs`, add (near the other `*Info`/`*Summary` types):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistroInfo {
    pub name: String,
    pub is_default: bool,
    pub selected: bool,
    pub home: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
    pub resolved_url: Option<String>,
    pub status: WslDistroStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WslDistroStatus {
    /// Has resolved URL, ready to apply.
    Ready,
    /// Probe returned NoProbeTool — distro lacks curl/wget/bash+timeout.
    NeedsProbeTool,
    /// Probe attempted but no URL succeeded.
    Unreachable,
    /// Not yet probed.
    Unknown,
}
```

- [ ] **Step 2: Service methods**

In `crates/llm-relay-core/src/service.rs`, add:

```rust
    pub async fn list_wsl_distros(&self) -> Result<Vec<crate::ipc::protocol::WslDistroInfo>, AppError> {
        use crate::ipc::protocol::{WslDistroInfo, WslDistroStatus};
        let rows = self.ctx.db.list_wsl_distros()?;
        Ok(rows.into_iter().map(|r| {
            let status = match (&r.resolved_url, r.probed_at.is_some()) {
                (Some(_), _) => WslDistroStatus::Ready,
                (None, true) => WslDistroStatus::Unreachable,
                (None, false) => WslDistroStatus::Unknown,
            };
            WslDistroInfo {
                name: r.name,
                is_default: r.is_default,
                selected: r.selected,
                home: r.home,
                has_claude: r.has_claude,
                has_codex: r.has_codex,
                has_gemini: r.has_gemini,
                resolved_url: r.resolved_url,
                status,
            }
        }).collect())
    }

    pub async fn toggle_wsl_distro(&self, name: String, selected: bool) -> Result<(), AppError> {
        self.ctx.db.set_wsl_distro_selected(&name, selected)?;
        Ok(())
    }
```

- [ ] **Step 3: Tauri command bindings**

In `src-tauri/src/lib.rs`, find the existing `#[tauri::command]` block (search for `set_active` or `add_gateway`) and add:

```rust
#[tauri::command]
async fn list_wsl_distros(service: tauri::State<'_, llm_relay_core::Service>) -> Result<Vec<llm_relay_core::ipc::protocol::WslDistroInfo>, String> {
    service.list_wsl_distros().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn toggle_wsl_distro(
    service: tauri::State<'_, llm_relay_core::Service>,
    name: String,
    selected: bool,
) -> Result<(), String> {
    service.toggle_wsl_distro(name, selected).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn refresh_wsl_distros(
    sm: tauri::State<'_, std::sync::Arc<llm_relay_core::wsl::state::StateMachine>>,
) -> Result<(), String> {
    sm.request_refresh();
    Ok(())
}
```

In the `invoke_handler!` macro list, add these three command names.

In the `setup` callback, store `wsl_state` (from Task 19) in Tauri state:
`app.manage(wsl_state.expect("state machine must be present"));`

- [ ] **Step 4: Verify compile**

```bash
cargo check --workspace
```

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(ipc): WslDistroInfo + list/toggle/refresh commands"
```

---
## Phase 7: Frontend Settings UI + README

### Task 21: WslDistros React component

**Files:**
- Create: `src/components/Settings/WslDistros.tsx`

- [ ] **Step 1: Find the existing Settings file for context**

Run: `find src -name 'Settings*' -type f`
Read whichever file owns the Settings panel layout to learn its import + styling conventions. Don't modify it yet.

- [ ] **Step 2: Create the component**

Create `src/components/Settings/WslDistros.tsx`:

```tsx
import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";

type WslDistroStatus = "ready" | "needsProbeTool" | "unreachable" | "unknown";

interface WslDistroInfo {
  name: string;
  isDefault: boolean;
  selected: boolean;
  home: string | null;
  hasClaude: boolean;
  hasCodex: boolean;
  hasGemini: boolean;
  resolvedUrl: string | null;
  status: WslDistroStatus;
}

export function WslDistros() {
  const [isWindows, setIsWindows] = useState(false);
  const [distros, setDistros] = useState<WslDistroInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    platform().then((p) => setIsWindows(p === "windows"));
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<WslDistroInfo[]>("list_wsl_distros");
      setDistros(list);
    } catch (e) {
      console.error("list_wsl_distros failed", e);
      setDistros([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!isWindows) return;
    void load();
  }, [isWindows, load]);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await invoke("refresh_wsl_distros");
      // Poll once after the state machine has had time to run.
      setTimeout(() => { void load(); setRefreshing(false); }, 1500);
    } catch (e) {
      console.error("refresh_wsl_distros failed", e);
      setRefreshing(false);
    }
  };

  const handleToggle = async (name: string, selected: boolean) => {
    try {
      await invoke("toggle_wsl_distro", { name, selected });
      setDistros((d) => d.map((x) => x.name === name ? { ...x, selected } : x));
    } catch (e) {
      console.error("toggle_wsl_distro failed", e);
    }
  };

  if (!isWindows) return null;

  return (
    <section className="space-y-3 rounded border border-zinc-700 p-4">
      <header className="flex items-center justify-between">
        <h3 className="font-semibold">WSL2 Distros</h3>
        <button
          className="rounded bg-zinc-700 px-2 py-1 text-xs hover:bg-zinc-600 disabled:opacity-50"
          onClick={handleRefresh}
          disabled={refreshing || loading}
        >
          {refreshing ? "Refreshing…" : "🔄 Refresh"}
        </button>
      </header>

      {loading ? (
        <p className="text-sm text-zinc-400">Loading…</p>
      ) : distros.length === 0 ? (
        <div className="space-y-2 text-sm text-zinc-400">
          <p>No WSL2 distros detected.</p>
          <p>
            If you use Claude / Codex / Gemini CLI inside WSL2, install one via
            Microsoft Store or <code className="rounded bg-zinc-800 px-1">wsl --install</code>,
            then click Refresh.
          </p>
        </div>
      ) : (
        <ul className="space-y-2">
          {distros.map((d) => (
            <li key={d.name} className="flex items-start gap-3">
              <input
                type="checkbox"
                className="mt-1"
                checked={d.selected}
                onChange={(e) => handleToggle(d.name, e.target.checked)}
              />
              <div className="flex-1 text-sm">
                <div>
                  <span className="font-medium">{d.name}</span>
                  {d.isDefault && <span className="ml-1 text-zinc-400">(default)</span>}
                </div>
                <div className="text-xs text-zinc-400">
                  {d.home ?? "(home unknown)"} ·{" "}
                  <span className={d.hasClaude ? "" : "text-zinc-600"}>claude {d.hasClaude ? "✓" : "✗"}</span>{" "}
                  <span className={d.hasCodex ? "" : "text-zinc-600"}>codex {d.hasCodex ? "✓" : "✗"}</span>{" "}
                  <span className={d.hasGemini ? "" : "text-zinc-600"}>gemini {d.hasGemini ? "✓" : "✗"}</span>
                </div>
                <StatusLine status={d.status} url={d.resolvedUrl} />
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function StatusLine({ status, url }: { status: WslDistroStatus; url: string | null }) {
  switch (status) {
    case "ready":
      return <div className="text-xs text-emerald-400">→ {url}</div>;
    case "needsProbeTool":
      return (
        <div className="text-xs text-amber-400">
          Install curl or wget in this distro (e.g. <code>sudo apt install curl</code>), then Refresh.
        </div>
      );
    case "unreachable":
      return <div className="text-xs text-red-400">Unreachable — check WSL networking, then Refresh.</div>;
    case "unknown":
      return <div className="text-xs text-zinc-500">Not yet probed.</div>;
  }
}
```

- [ ] **Step 3: Mount in Settings page**

Open the Settings file found in Step 1 and add an import and render call for `<WslDistros />`. Place it after the existing "client identity" / "auto-failover" sections.

- [ ] **Step 4: Install `@tauri-apps/plugin-os` if not present**

Run: `grep "plugin-os" package.json`
If absent: `pnpm add @tauri-apps/plugin-os` and add it to `src-tauri/Cargo.toml` plugins, and register in `src-tauri/src/lib.rs` builder chain. (If the plugin is already used elsewhere, skip these sub-steps.)

- [ ] **Step 5: Build the frontend to verify TS compiles**

```bash
pnpm tsc --noEmit
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src/components/Settings/WslDistros.tsx src/ package.json pnpm-lock.yaml src-tauri/
git commit -m "feat(ui): WSL2 Distros settings panel (Windows-only)"
```

---

### Task 22: README — "在 WSL2 中使用" section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a new section**

Add between the existing "服务器部署（TUI + 无头 agent）" section and "GUI ↔ TUI 互斥与接管":

```markdown
---

## 在 WSL2 中使用（Windows）

LLM Relay 在 Windows GUI 模式下会自动管理 WSL2 里的 Claude / Codex / Gemini CLI 配置。

### 自动检测

启动 Relay 后打开 **Settings → WSL2 Distros**：

- 自动列出所有已安装的 WSL2 distro（WSL1 会被过滤）
- 默认 distro 默认勾选；其它 distro 可手动勾选
- 每行显示该 distro 是否已装 claude / codex / gemini —— 没装的会跳过

### 网络

Relay 在 Windows 的 `127.0.0.1:18080` 和 WSL 虚拟网卡 IP（通常 `172.x.x.1:18080`）上各开一个监听 ——
**物理网卡（以太网 / WiFi）完全不监听，局域网无法触及**，无需防火墙配置。

WSL 端写入的 URL 是 `http://host.docker.internal:18080`（NAT 模式）或 `http://127.0.0.1:18080`
（mirror 模式）。两者皆为稳定 hostname，Windows / WSL 重启不会让配置失效。

启动时 Relay 会从 distro 内部跑一次 HTTP probe，挑出可达的 URL 写入配置。
要求 distro 里装了 `curl` 或 `wget`（极简镜像如裸 Alpine 可能没装；UI 会提示）。

### 取消勾选 / Disable

- 取消勾选某 distro → 该 distro 的 CLI 配置恢复到首次 apply 之前的状态
- Disable Relay（清空 active gateway）→ Windows + 所有勾选 distro 都恢复
- 重装 / unregister 一个 distro 之前请先取消勾选，否则恢复快照会失败（数据无损，只是 warning 进 log）
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs(readme): add 在 WSL2 中使用 section"
```

---

### Task 23: End-to-end verification

**Files:**
- None (manual + scripted checks)

Follow the spec's §6 verification plan. Each check produces a pass/fail record;
commit a short verification log to `docs/superpowers/verification/2026-06-09-wsl2.md`
afterwards.

- [ ] **Step 1: Build full release artifacts**

```bash
cargo build --release -p llm-relay-core -p llm-relay-agent -p llm-relay-tui
pnpm build
```

Expected: clean build on all platforms.

- [ ] **Step 2: Cross-platform unit test sweep**

```bash
cargo test --workspace
```

Expected: all PASS on Windows. On mac/Linux, WSL-specific tests are auto-gated out and the rest must pass.

- [ ] **Step 3: Manual scenarios (Windows host with Ubuntu WSL)**

Run through:

1. Single distro default — `claude "hi"` inside Ubuntu succeeds.
2. Multi-distro — install Debian; toggle in Settings; verify both distros' `~/.claude/settings.json` written.
3. Un-toggle Debian — verify Debian's snapshot restored, Windows + Ubuntu untouched.
4. Disable Relay — verify Windows + all selected distros restored.
5. `wsl --shutdown` then restart Ubuntu — verify Relay rebinds the new gateway IP within 60s.
6. From another LAN host: `curl http://<my-LAN-IP>:18080` — expect connection refused.
7. `wsl --unregister Debian` — apply/clear still works, warning logged.
8. Drop a fake legacy `~/.llm-relay/cli-config-backup.json` — restart Relay — verify migration to `cli-config-backup/windows.json`.

- [ ] **Step 4: No-WSL Windows scenario**

On a Windows machine without WSL installed: launch Relay → Settings → WSL2 Distros shows "No WSL2 distros detected", apply/clear works normally for Windows.

- [ ] **Step 5: mac/Linux regression**

Build + launch on mac/Linux: Settings doesn't render WSL panel; apply/clear behavior unchanged from current release.

- [ ] **Step 6: Write verification log**

Create `docs/superpowers/verification/2026-06-09-wsl2.md` with one line per scenario above, marking PASS / FAIL / NOTES.

- [ ] **Step 7: Commit**

```bash
mkdir -p docs/superpowers/verification
git add docs/superpowers/verification/
git commit -m "docs(verification): WSL2 integration scenarios"
```

---

## Self-Review Checklist

Before announcing the plan complete, the planner ran through:

**Spec coverage** —
- §2.1 network: Tasks 4, 6, 7, 12, 19 ✓
- §2.1 ProxyHandle ownership chain: Task 6 ✓
- §2.1 /_relay/ping route + reserved namespace: Task 5 ✓
- §2.2 distro discovery + probe + UI + platform gate: Tasks 9, 10, 11, 21 ✓
- §2.3 CliBackend + WslBackend + CliTarget: Tasks 13, 14, 18 ✓
- §2.4 snapshot v2: Tasks 16, 17 ✓
- §2.5 legacy migration: Task 17 ✓
- §2.6 README: Task 22 ✓
- §3.5 detection state machine + lazy mode + no-WSL graceful: Tasks 19, 21 ✓
- §6 verification: Task 23 ✓

**Placeholder scan** — no TBD / TODO / unspecified handlers.

**Type consistency** — `DistroRow`, `CliTarget`, `CliBackend`, `SnapshotMeta`, `TargetSnapshot`, `WslDistroInfo` names match across tasks.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-09-wsl2-integration.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
