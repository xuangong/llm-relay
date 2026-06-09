# WSL2 集成设计

**日期**：2026-06-09
**状态**：Spec (待用户确认)
**适用范围**：Windows-only 增量功能；mac/Linux 行为不变。

---

## 1. 背景

LLM Relay 当前作为 Windows 桌面应用运行：

- 本地代理监听 `127.0.0.1:18080`
- CLI 配置写入 Windows 用户目录（`C:\Users\<user>\.claude\`、`.codex\`、`.gemini\`）

用户也在同一台机器的 WSL2 里跑 Claude Code / Codex / Gemini CLI。WSL2 场景下当前架构两个问题都不解决：

1. **网络层**：WSL2 默认 NAT 模式下，`127.0.0.1` 是 WSL2 自己的 loopback，够不到 Windows 主机
2. **文件层**：WSL2 里的 CLI 读 Linux 路径 `/home/<user>/.claude/...`，Windows 端写的配置完全不可见

本设计在不影响 Windows 原生使用、不破坏 mac/Linux 平台的前提下，让 WSL2 里跑的 CLI 工具也能透明使用 LLM Relay。

---

## 2. 设计要点

### 2.1 网络层：精准 bind + `host.docker.internal`

Relay 同时 bind 两个具体接口（**不**用 `0.0.0.0`，避免 LAN 暴露和防火墙弹窗）：

- `127.0.0.1:18080` — Windows 本机访问
- `<WSL2 网关 IP>:18080` — WSL2 通过虚拟网卡访问

WSL2 网关 IP 是 Windows 上虚拟网卡 `vEthernet (WSL)` 的 IPv4，通常形如 `172.x.x.1`，**每次 Windows / WSL 重启会变**。

写入 CLI 配置的 URL：

| Target | URL（默认） | 备选 |
|---|---|---|
| Windows | `http://127.0.0.1:18080` | — |
| WSL2 | `http://host.docker.internal:18080` | `http://127.0.0.1:18080`（mirror 模式下） |

**重要约束**：CLI 配置是 **write-once**（apply 时写入，gateway 切换不会重写）。因此 WSL 端 URL **必须使用稳定标识符**——不能写裸 IP（Windows / WSL 重启时 WSL 网关 IP 会变，已写入的配置会失效）。可选稳定标识符只有两个：

1. **`host.docker.internal`** — WSL 0.65+ 默认在 `/etc/hosts` 注入；NAT 模式下解析到 WSL 网关 IP，mirror 模式下解析到 `127.0.0.1`；用户若关掉了 `[network] generateHosts` 则不存在
2. **`127.0.0.1`** — 仅在 mirror 模式下能到达 Windows

#### Per-distro URL 探测

apply 时（不是 Relay 启动时）对每个勾选的 distro 跑一次探测，**连通性验证是强制的**——只验证 DNS 解析不够，因为名字解析到一个不可达地址会写出永久坏掉的 URL（配置 write-once）。

前置条件：本步骤跑之前，§2.1 末尾的 `/_relay/ping` 路由（轻量 200 OK，无副作用）必须已挂在 proxy 上，且 Relay 已 bind `127.0.0.1:18080` + `<WSL gateway IP>:18080`（若有）。

**`/_relay/ping` 路由实现要点**（在 `proxy_server.rs` 的 `Router` 上）：

```rust
async fn relay_ping() -> &'static str { "ok" }
async fn relay_reserved() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "unknown relay endpoint")
}

let app = Router::new()
    .route("/_relay/ping", get(relay_ping))
    // 兜底所有 _relay/* 路径，**必须**显式注册 — 否则未知的 _relay/* 会落到
    // .fallback(forward) 被当成上游请求转给 gateway，违背 "reserved namespace" 约定
    .route("/_relay/{*rest}", any(relay_reserved))
    .fallback(forward)
    .with_state(state);
```

- 必须**先** route 再 fallback，否则会被吞进 `forward` 转发到上游 gateway（会 404 或更糟，被上游记错请求）
- `_relay/*` 兜底返回本地 404，永不转发上游
- 不进 `traffic_log` / `usage_log` / `consecutive_errors` 计数——由路由分流天然实现（这些计数在 `forward` 内部，`_relay/*` handler 跟它们物理上无交集）
- 响应纯字符串，不做鉴权（local-only 监听，没有威胁）
- 后续若加 `/_relay/version` 等：在 `relay_reserved` 兜底前显式注册即可

每个候选 URL **都跑一次真实 HTTP probe**。最小 distro 可能既没 curl 也没 wget，所以显式 fallback，二者全无时跳到最后一招用 `/dev/tcp`（bash builtin，几乎所有 distro 都有）：

```sh
# 从 distro 内部探测，验证 Windows 上的 Relay 监听确实可达
wsl -d <D> -e sh -c '
  probe() {
    url="$1"
    if command -v curl >/dev/null 2>&1; then
      code=$(curl -fsS -o /dev/null -w "%{http_code}" --max-time 2 "$url/_relay/ping" 2>/dev/null)
      [ "$code" = "200" ]
    elif command -v wget >/dev/null 2>&1; then
      wget -q -O /dev/null --timeout=2 --tries=1 "$url/_relay/ping" 2>/dev/null
    else
      # bash /dev/tcp fallback — 只验 TCP 通，不验 200，但比误判 unreachable 强
      host=$(echo "$url" | sed -E "s|http://([^:/]+).*|\\1|")
      port=$(echo "$url" | sed -E "s|http://[^:]+:([0-9]+).*|\\1|")
      timeout 2 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null
    fi
  }
  for url in "http://host.docker.internal:18080" "http://127.0.0.1:18080"; do
    if probe "$url"; then echo "ok $url"; exit 0; fi
  done
  echo "unreachable"
  exit 1
'
```

不把 curl 当依赖文档化——某些 server-oriented distro（Alpine、distroless 衍生品）真的没装。`/dev/tcp` 路径只能验 TCP 不能验 200，可能把"监听了但不是 Relay"的情况判通；考虑到本机 18080 端口被无关进程占用的概率极低，这个 trade-off 可接受。

选择规则（**严格按返回顺序**）：

1. 第一个 probe 返回成功的 URL 即为该 distro 的 `resolved_url`。"成功" 定义：
   - curl / wget 路径：HTTP 200
   - `/dev/tcp` fallback 路径：TCP 三次握手成功（无 HTTP 验证）
2. `host.docker.internal` 优先于 `127.0.0.1`（前者在两种模式都通；mirror 模式下两者都通，前者仍然正常工作，不需要区分模式）
3. 都不通 → `resolved_url = NULL`，UI 标红 "Unreachable"，apply 时该 distro 被跳过，warning 进 events
4. 探测失败的常见原因：WSL <0.65 且 `generateHosts=false`、用户改了 `/etc/hosts`、Windows 防火墙拦了 WSL adapter（罕见，因为是 loopback 类接口）
5. `/dev/tcp` 路径的退化风险：若本机 18080 被无关进程占用，会被误判为 reachable。Relay 启动时已独占 18080（lifecycle 锁），所以实操中只有"Relay 没启动却跑了 distro probe"这种顺序错误才会触发——属于调用方 bug 而非数据风险

不再用 `getent hosts` 或 `/proc/sys/kernel/osrelease` 做间接推断——它们只能证明"名字存在"或"是 WSL2"，不能证明"我能 HTTP 打到 Windows"。Mirror 模式的判定也归约成 "127.0.0.1 能 HTTP 通"，不再查内核版本。

探测结果（resolved URL）随 distro 缓存到 `wsl_distros.resolved_url` 字段。Refresh 按钮重跑探测。

#### Relay listener bind

无论 distro 探测结果如何，Relay 启动时按以下规则 bind：

- 必选：`127.0.0.1:18080`（Windows target + mirror 模式 WSL target）
- 可选：WSL2 网关 IP `<172.x.x.1>:18080`（NAT 模式 WSL target 通过 `host.docker.internal` 来）
- 启动时枚举 Windows 网卡找 `vEthernet (WSL)`，找到 → bind；找不到 → 跳过
- 周期重检测（见 §3.5 状态机）：网关 IP 变化 → cancel 旧 WSL listener task，spawn 新的；`127.0.0.1` listener task 永不动

#### Listener 生命周期与 ProxyHandle 所有权

当前 `proxy_server::start` 是一个长期运行的 `axum::serve` 任务，没有外部句柄。新设计要求**多 listener + 可重 bind**，所以重构成"启动返回句柄"模式：

```rust
// proxy_server.rs
pub struct ProxyHandle {
    primary_token: CancellationToken,         // 127.0.0.1 — 关 Relay 时一并 cancel
    primary_join:  JoinHandle<()>,            // shutdown 时 await，确保 serve task 真正退出
    wsl_listener:  Mutex<Option<WslBound>>,   // 可热替换
    state:         Arc<ProxyState>,           // spawn 新 listener 时复用
}

struct WslBound {
    ip:    IpAddr,
    token: CancellationToken,
    join:  JoinHandle<()>,
}

// 每个 listener spawn 时的标准模式：
async fn run_listener(listener: TcpListener, state: Arc<ProxyState>, token: CancellationToken) {
    let app = build_router(state);  // 见 §"/_relay/* 路由"
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { token.cancelled().await })
        .await
        .ok();  // serve 退出（正常 shutdown 或 listener accept 失败）→ task 自然结束
}
// CancellationToken 单独 cancel 不会停 serve；必须配合 with_graceful_shutdown 才能让
// serve 在 token 触发时停止 accept 新连接并优雅退出。

impl ProxyHandle {
    /// new_ip = None → 仅取消 WSL listener；Some → 取消旧的（如有）+ bind 新 IP + spawn
    pub async fn rebind_wsl(&self, new_ip: Option<IpAddr>) -> Result<(), AppError>;
    pub async fn shutdown(&self);  // cancel primary + wsl
}

pub async fn start_with_listeners(
    state: Arc<ProxyState>,           // 解耦 Service 依赖：直接传 state，不传 Service
    primary: TcpListener,             // 127.0.0.1，必选
    initial_wsl: Option<(IpAddr, TcpListener)>,  // 可选，启动时若 WSL 网卡可用就传
) -> Arc<ProxyHandle>;
```

**所有权链**（避免 Service ↔ ProxyHandle 构造循环）：

`ProxyState` 持有 `db / sink / switch_lock`——这些今天就在 `Service` 内，但可独立构造。改造：

1. **拆分构造**：把 `ProxyState` 字段抽成一个 `CoreContext { db, sink, switch_lock }` 结构，`Service` 和 `ProxyState` 都通过 `Arc<CoreContext>` 引用相同数据，不互相 own
2. **启动顺序**：
   - lifecycle 先建 `Arc<CoreContext>`
   - bind listeners
   - `proxy_server::start_with_listeners(Arc::new(ProxyState::from(ctx.clone())), primary, wsl)` → `Arc<ProxyHandle>`
   - 用 `ctx.clone()` 和刚拿到的 `proxy` 一起组装 `Service { ctx, proxy: Arc<ProxyHandle> }`
3. 状态机后台 task 拿 `service.proxy.clone()`；Tauri/TUI 命令同样
4. 关 Relay：`Service::shutdown` → `proxy.shutdown()`：
   - cancel `primary_token` + WSL token（若有）→ axum::serve 在下一次 accept 前观察到 cancellation 退出
   - await `primary_join` + WSL `join`，确保 serve task 真正结束（不只是 cancel 信号发出）
   - 释放最后一份 `Arc<CoreContext>`

这种"先 ctx，后 proxy，后 service"的线性构造，比 `OnceCell<Arc<ProxyHandle>>` 干净：Service 一旦存在 `proxy` 字段就已是非 None，调用方不用处理"还没初始化"分支。

**WSL bind 失败处理**：`rebind_wsl(Some(ip))` 内部 bind 报错（端口已占 / IP 已消失）→ `wsl_listener` 保持 `None` + 返回 `Err`，调用方记 warning 进 events；`primary_token` 不受影响，Relay 仍为 Windows target 可用。

WSL bind 失败（端口已被占 / IP 已消失）→ warning 进 events，`127.0.0.1` listener 不受影响，Relay 继续可用。

#### 物理网卡上完全没有 listener
- 同 WiFi/办公网的别人 SYN 不会被接受 → 0 LAN 暴露
- 不需要任何防火墙规则、不需要 token

### 2.2 Distro 管理：用户多选，默认勾选默认 distro

#### 发现
- `wsl.exe -l -v --quiet` 列所有已注册 distro + 运行状态
- `wsl.exe --status` 拿默认 distro 名

#### Probe（每个 distro 一次性 + 手动刷新触发）
对每个 distro 拉取：
- `home` ← `wsl -d <D> -e sh -c 'echo $HOME'`
- `user` ← `wsl -d <D> -e whoami`
- `has_claude` / `has_codex` / `has_gemini` ← 一次 shell 调用，独立检查每个 binary（不能用 `&&` 串接——会短路）：
  ```sh
  for c in claude codex gemini; do
    if command -v "$c" >/dev/null 2>&1; then echo "$c=1"; else echo "$c=0"; fi
  done
  ```
- `resolved_url` ← 见 §2.1 per-distro URL 探测

结果写入 SQLite。新增表：

```sql
CREATE TABLE wsl_distros (
  name          TEXT PRIMARY KEY,
  is_default    INTEGER NOT NULL DEFAULT 0,
  selected      INTEGER NOT NULL DEFAULT 0,  -- 用户勾选
  home          TEXT,                         -- probe 缓存
  user          TEXT,
  has_claude    INTEGER NOT NULL DEFAULT 0,
  has_codex     INTEGER NOT NULL DEFAULT 0,
  has_gemini    INTEGER NOT NULL DEFAULT 0,
  resolved_url  TEXT,                         -- §2.1 探测出的可用 URL，NULL=未探测/不可达
  probed_at     TEXT
);
```

#### UI（Settings 页面）
Windows 平台下新增一块 "WSL2 Distros"：

```
WSL2 Distros                                     [ 🔄 Refresh ]

  ☑ Ubuntu (default)
      /home/xanzh · claude ✓ codex ✓ gemini ✗

  ☐ Debian
      /home/user · claude ✗ codex ✗ gemini ✗  [Not installed]

  ☐ Ubuntu-22.04
      /home/dev · claude ✓ codex ✓ gemini ✓
```

- 首次安装/首次进入 Settings：自动 probe；默认 distro 默认勾选，其余不勾
- CLI 全部 ✗ 的 distro 仍可勾选（但 apply 时三个 backend 都 skip，等价于不写）

#### 平台门控
- 整个 WSL 模块用 `#[cfg(target_os = "windows")]` 包裹
- mac/Linux 上 Settings 页不渲染该板块（前端用 Tauri `platform()` 判断）
- 数据库表所有平台都建（无害），但只有 Windows 写入

### 2.3 文件读写层：`wsl.exe` 子进程 backend

新增模块 `crates/llm-relay-core/src/wsl/`：

```
wsl/
  mod.rs        - public API
  distro.rs     - 发现、probe、缓存
  fs.rs         - wsl.exe 子进程读/写/删
  network.rs    - WSL 网关 IP 发现
```

#### `fs.rs` 提供的 API

```rust
pub fn wsl_read(distro: &str, path: &str) -> Result<Option<String>, AppError>;
pub fn wsl_atomic_write(distro: &str, path: &str, bytes: &[u8]) -> Result<(), AppError>;
pub fn wsl_remove(distro: &str, path: &str) -> Result<(), AppError>;
pub fn wsl_exists(distro: &str, path: &str) -> Result<bool, AppError>;
```

#### `wsl_atomic_write` 实现

```rust
// distro 内部用 mktemp + mv -f 做 atomic rename
// 避免 Windows / WSL 互操作的 race；stdin 喂内容避开 shell 转义
let script = r#"
  umask 077
  d="$(dirname "$1")"
  mkdir -p "$d"
  t="$(mktemp "$d/.llmrelay.tmp.XXXXXX")"
  cat > "$t"
  mv -f "$t" "$1"
"#;

Command::new("wsl.exe")
  .args(["-d", distro, "-e", "sh", "-c", script, "_", path])
  .stdin(Stdio::piped())
  // ... 写 bytes 到 stdin
```

#### Backend trait + Target 抽象（重构 `config_writer.rs`）

文件读写和 base_url 是两个独立维度：Windows 和 WSL backend 各自定义"配置文件在哪个文件系统、按什么路径写"，而 base_url 由探测决定且 per-target 不同（Windows = `127.0.0.1:18080`，每个 WSL distro 用自己的 `resolved_url`）。因此拆成两层：

```rust
trait CliBackend {
    fn read(&self, rel_path: &[&str]) -> Result<Option<String>, AppError>;
    fn write_atomic(&self, rel_path: &[&str], bytes: &[u8]) -> Result<(), AppError>;
    fn remove(&self, rel_path: &[&str]) -> Result<(), AppError>;
    fn exists(&self, rel_path: &[&str]) -> Result<bool, AppError>;
}

struct WindowsFsBackend;
struct WslBackend { distro: String, home: String }

pub struct CliTarget {
    pub backend: Box<dyn CliBackend + Send + Sync>,
    pub base_url: String,                    // 探测出的可用 URL
    pub installed: InstalledTools,           // { claude, codex, gemini } booleans
    pub label: String,                       // "windows" / "wsl:Ubuntu" — 日志、snapshot inner field
    pub snapshot_meta: SnapshotMeta,         // { target_type, distro_name } — 写入 snapshot JSON
}

pub struct InstalledTools { pub claude: bool, pub codex: bool, pub gemini: bool }
```

`apply_all_configs` 改成接收 `&[CliTarget]`，对每个 target 跑：
- `if target.installed.claude { write_claude_config(&*target.backend, &target.base_url, api_key, claude_model, claude_small_model)?; }`
- 三个 `write_*_config` 函数签名改成 `fn write_xxx(backend: &dyn CliBackend, base_url: &str, api_key: &str, ...)`，把 `base_url` 显式传入（今天是全局函数从 `proxy_base_url()` 隐式取）
- `installed.* = false` 的 CLI 自动跳过——Windows target 三个全 true，WSL target 按 probe 结果决定

构造 target 的逻辑（在 `Service::build_apply_targets()` 里）：
- Windows target：`base_url = "http://127.0.0.1:18080"`，`installed = {true, true, true}`（Windows 端没装 CLI 也照写，与今天行为一致）
- 每个 selected distro：`base_url = wsl_distros.resolved_url`（NULL → 跳过该 distro），`installed` 来自 `has_claude / has_codex / has_gemini`

这样彻底解决"WSL 端被写成 127.0.0.1"的问题——`base_url` 跟着 target 走，不再是全局值。

### 2.4 Apply / Clear / Snapshot

#### Snapshot 目录布局

```
~/.llm-relay/cli-config-backup/
  windows.json              ← Windows 本机
  wsl-<opaque-id>.json      ← per-distro，文件名是不透明 id
  wsl-<opaque-id>.json
```

**文件名是不透明 id**（避免 distro 名带空格/特殊字符引发碰撞，例如 `Ubuntu 22.04` vs `Ubuntu_22.04`）：

```
opaque_id = base32(sha256(utf8(distro_name)))[:16].lower()
```

restore 时**不依赖文件名解析 distro 名**，而是从 JSON 内容里读：

```jsonc
{
  "target_type": "wsl",           // "windows" | "wsl"
  "distro_name": "Ubuntu 22.04",  // 原始名，restore 时直接传给 wsl -d
  "home": "/home/dev",            // probe 时的 home，避免 restore 时再 probe 一次
  "captured_at": "2026-06-09T...",
  "claude": { ... },
  "codex":  { ... },
  "gemini": { ... }
}
```

`windows.json` 同样有 `target_type: "windows"`（迁移时补上）。

#### `apply_all_configs` 改写

```
targets = [Windows]
        + 每个 selected distro 的 WslBackend

# index: distro_name → snapshot 文件路径
# 启动时扫一遍 snapshot 目录，读每个文件的 distro_name 字段建索引
prev_index = build_index(snapshot_dir)

新加入的 target（distro_name 不在 prev_index）→ 先 capture_snapshot，再写入
保留的 target（在 prev_index）→ 直接写入（不重新 capture）
被剔除的 target（在 prev_index 不在 current）→ 用 snapshot restore，删除 snapshot 文件
```

#### `clear_all_configs`（Disable）
- 遍历 snapshot 目录所有 `*.json`
- 读每个文件的 `target_type` + `distro_name`，构造对应 backend（`windows` → `WindowsFsBackend`，`wsl` → `WslBackend{distro: <原始名>, home: <snapshot.home>}`）
- 对每个 backend 执行 restore
- 删除整个目录

#### 错误隔离
- 某 distro 停了、被 unregister、`wsl.exe` 超时（超过 5s）→ 该 target 记一条 warning 到 events，不影响其它 target
- Apply 整体成功定义：**至少 Windows target 成功**；WSL target 失败仅 warning

### 2.5 老用户迁移

升级到带 WSL2 支持的版本时，启动时一次性。**不能只 rename**——新格式要求顶层有 `target_type: "windows"` 字段，老文件没有，clear 路径会拒绝。流程：

```rust
let old_path = config_dir().join("cli-config-backup.json");
let new_dir  = config_dir().join("cli-config-backup");
let new_path = new_dir.join("windows.json");

if old_path.exists() && !new_path.exists() {
    fs::create_dir_all(&new_dir)?;

    // 1) 读老 JSON（CliConfigSnapshot 结构，没有 target_type）
    let old_bytes = fs::read(&old_path)?;
    let mut v: serde_json::Value = serde_json::from_slice(&old_bytes)?;

    // 2) 补字段：target_type + distro_name（None for windows）
    let obj = v.as_object_mut()
        .ok_or_else(|| AppError::Config("legacy snapshot is not a JSON object".into()))?;
    obj.insert("target_type".into(), json!("windows"));
    // distro_name 字段留空 / 不存在，反序列化端用 Option

    // 3) 原子写新文件
    let new_bytes = serde_json::to_vec_pretty(&v)?;
    atomic_write(&new_path, &new_bytes)?;

    // 4) 删旧文件（成功才删，失败下次启动重试）
    fs::remove_file(&old_path)?;
}
```

幂等：`new_path` 已存在就不动；老文件不存在就不做事。读旧 JSON 失败（格式损坏）→ rename 旧文件加 `.corrupt` 后缀，记 warning，跳过迁移（避免每次启动重复失败）。

### 2.6 文档

README 新增一节 "在 WSL2 中使用"，说明：
- 默认 distro 自动支持，其它 distro 在 Settings 里勾选
- WSL2 端配置自动写入 `~/.claude/settings.json` 等 Linux 路径
- 用 `host.docker.internal` 而非 IP（不受 WSL 重启影响）
- 重装 distro 前请先在 Settings 里取消勾选或 Disable

---

## 3. 改动文件清单

| 文件 | 改动 |
|---|---|
| `crates/llm-relay-core/src/config_writer.rs` | 拆出 `CliBackend` trait；现有函数全部改为 `&dyn CliBackend` 入参；保留 Windows 路径行为 |
| `crates/llm-relay-core/src/wsl/mod.rs` | 新增（Windows-only） |
| `crates/llm-relay-core/src/wsl/distro.rs` | distro 发现 + probe + 缓存读写 |
| `crates/llm-relay-core/src/wsl/fs.rs` | `wsl_read` / `wsl_atomic_write` / `wsl_remove` / `wsl_exists` |
| `crates/llm-relay-core/src/wsl/network.rs` | WSL 网关 IP 发现 + 变更检测 |
| `crates/llm-relay-core/src/proxy_server.rs` | 接收多个 listener；启动时叠加 WSL 网关 IP listener；周期重 bind |
| `crates/llm-relay-core/src/lifecycle.rs` | 启动时 bind WSL 网关 IP 的 std listener，与 127.0.0.1 listener 一起传给 proxy |
| `crates/llm-relay-core/src/database.rs` | 加 `wsl_distros` 表 migration |
| `crates/llm-relay-core/src/paths.rs` | snapshot 路径从单文件改为目录；老用户迁移函数 |
| `crates/llm-relay-core/src/service.rs` | 启动时跑迁移；apply/clear 走新流程 |
| `src/components/Settings.tsx`（或对应文件） | WSL Distros 板块（仅 Windows 显示） |
| `src-tauri/src/lib.rs` | 暴露 distro 列表 / refresh / toggle 命令 |
| `README.md` | "在 WSL2 中使用" 一节 |

---

## 3.5 WSL2 未安装/未启用时的行为

Windows 可能根本没装 WSL2、装了但没装任何 distro、或装了但被禁用。所有这些场景下 LLM Relay 必须**静默降级**到今天的纯 Windows 行为，不阻塞、不报错、不弹窗。

### 网络层
- `find_wsl_gateway_ip()` 枚举 adapter 找不到 `vEthernet (WSL)` → 返回 `None`
- proxy 只 bind `127.0.0.1:18080`，与现状完全一致
- 周期重检测的频率取决于 §3.5 末尾的状态机（Active 模式 60s；Lazy 模式仅手动 Refresh）

### Distro 发现
`wsl.exe -l -v` 在不同场景下的失败形式：

| 场景 | 表现 |
|---|---|
| 完全没装 WSL（连 `wsl.exe` 都没） | `Command::new("wsl.exe")` 报 `ENOENT` / "program not found" |
| 装了 WSL 但没装 distro | 命令成功，stdout 是 "Windows Subsystem for Linux has no installed distributions" |
| 装了但被禁用（Windows feature 关了） | 返回非零退出码 + 特定错误信息 |
| `wsl.exe` 超时（罕见） | 5s 超时主动 kill |

**所有失败统一视为 "no available distros"**：distro 列表为空，不报错，不区分子类型。

### UI
WSL2 板块在 distro 列表为空时显示：

```
WSL2 Distros
─────────────────────────────────────────────
No WSL2 distros detected.

If you use Claude/Codex/Gemini CLI inside WSL2,
install it via Microsoft Store or `wsl --install`.

[ 🔄 Re-check ]
```

不弹错误对话框、不阻塞 apply、不影响 Windows 端任何功能。

### Apply / Clear
- 没有 selected distro → `apply_all_configs` 只迭代 `[Windows]` target
- snapshot 目录里只有 `windows.json`，行为与今天一致

### 检测状态机（gateway IP + distro 列表共用）

唯一的 WSL 检测 cadence 状态机，避免 §2.1 / §3.5 之间的不一致：

- **启动时**：跑一次完整检测（`wsl -l -v` + `find_wsl_gateway_ip`）
- 若 "WSL 可用且有 distro" → **Active** 模式：每 60s 重跑（与健康检查 tick 搭车）
  - distro 列表变化 → 更新 UI；新 distro 自动 probe
  - gateway IP 变化 → 通过 `ProxyHandle::rebind_wsl` 重 bind
- 否则 → **Lazy** 模式：不再周期检测，仅在以下时机重跑：
  - 用户点 🔄 Re-check
  - 应用重启
  - 用户在 Settings 里点了任何 WSL 相关操作（implicit refresh）
- Lazy → Active 的转换：用户事后启用 WSL 后点 Refresh，检测成功 → 切到 Active

这保证零 WSL 用户的开销近 0（仅启动时一次 ~50ms 的 `wsl.exe -l -v`），同时启用 WSL 后无需重启 Relay（点一次 Refresh 即可）。

---

## 4. 不在范围内

- **WSL1**：不支持。`wsl -l -v` 区分版本号，version=1 的 distro 直接过滤掉
- **远程 WSL**（SSH 到另一台机器的 WSL）：完全不在范围
- **写入 distro 内部的 shell rc**（如 `~/.bashrc` 加 `OPENAI_API_KEY=dummy`）：第一版不做，仅写 `~/.codex/auth.json`；如果 Codex CLI 在 WSL 里报缺 env，再加
- **多用户 distro**：probe 用 `whoami`，只支持当前 wsl 启动的默认用户；多用户切换不处理
- **强制 mirror 模式**：不强制，§2.1 探测会自动选可用 URL；mirror 模式下 `127.0.0.1` 仅作为 fallback

---

## 5. 风险与缓解

| 风险 | 缓解 |
|---|---|
| `host.docker.internal` 在 WSL 0.65 以下不存在 / 用户关掉了 `generateHosts` | §2.1 per-distro URL 探测会自动 fallback 到 mirror 模式的 `127.0.0.1`。两者都不可达 → UI 标红"Unreachable"但 checkbox 不禁用，用户可改配置后 Refresh |
| WSL 网关 IP 变更未及时检测 → WSL 端请求超时 | Active 模式 60s 重检测自动 rebind；Settings 加 "Reconnect WSL" 按钮立即触发 |
| `wsl.exe -e cat` 启动冷 distro 慢（1-3s） | apply 整体异步；UI 显示 per-target 状态 |
| 用户在 WSL 里手动改了配置后 Disable 会被覆盖 | snapshot 保的是"首次 apply 前"状态。文档说明：手动改过的 WSL 配置请在 Disable 前自行备份 |
| Distro 名含奇怪字符（中文、空格、`/`） | snapshot 文件名走 sha256 不透明 id；原始名存 JSON 内，`wsl -d <D>` 用原始名 |
| Tauri 安装包不附带 WSL 二进制依赖 | 完全不依赖，`wsl.exe` 是 Windows 自带 |

---

## 6. 验证计划

实现完成后手动验证：

1. **单 distro 默认场景**：装一个 Ubuntu 当默认 distro，启用 Relay，在 WSL 跑 `claude "hi"` 能通
2. **多 distro 勾选**：勾选 Ubuntu + Debian，apply，检查两个 distro 的 `~/.claude/settings.json` 都被写入
3. **取消勾选**：取消 Debian，apply，确认 Debian 的配置被 restore，Windows + Ubuntu 不受影响
4. **Disable 全局**：关闭 Relay，确认 Windows + 所有勾选 distro 都恢复到首次 apply 前的状态
5. **WSL 重启**：`wsl --shutdown` 然后重启 distro，确认 60s 内 Relay 重新 bind 新网关 IP，WSL 端请求恢复
6. **LAN 隔离**：另一台机器 `curl http://<本机 LAN IP>:18080`，确认 connection refused（物理网卡无 listener）
7. **WSL 卸载**：unregister 一个已勾选 distro，apply/clear 时该 target 报 warning 但流程继续
8. **老用户迁移**：mock 一个 `cli-config-backup.json`，启动后确认自动迁移到 `cli-config-backup/windows.json`
9. **mac/Linux 回归**：在非 Windows 平台跑一遍，确认 Settings 不渲染 WSL 板块、apply/clear 行为零变化
10. **无 WSL 的 Windows**：在没装 WSL 的 Windows 上跑，确认 Settings 显示 "No WSL2 distros detected" + 安装指引，无任何报错，Windows 端 apply/clear 完全正常
11. **WSL 事后启用**：从无 WSL 状态启动 Relay → 安装 WSL + distro → 点 🔄 Re-check 后 distro 出现在列表里，可勾选可 apply
