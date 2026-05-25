# Release Readiness Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the project internally consistent and safer to publish by fixing release docs, CI release assets, version checks, license, and verification notes.

**Architecture:** This is a release-engineering pass only. It changes documentation, GitHub Actions packaging, and small helper scripts; it does not change runtime app behavior.

**Tech Stack:** Tauri 2, pnpm, Rust workspace/Cargo, GitHub Actions, bash-compatible Node script.

---

## File Map

- Modify: `packaging/systemd/README.md` — align headless agent setup with `LLM_RELAY_MASTER_KEY` env keystore.
- Modify: `packaging/systemd/llm-relay-agent.service` — replace placeholder Documentation URL and add explicit master-key guidance.
- Modify: `README.md` — replace clone placeholder, add TUI/agent release asset names, add smoke-test checklist link/section.
- Modify: `BUILD.md` — replace clone placeholders, update Tauri 2 docs link, clarify CI/manual release asset scope, include TUI/agent assets and smoke tests.
- Modify: `RELEASE_NOTES.md` — update download table with TUI/agent assets, keep manual/unsigned warning clear, make test status current after verification.
- Modify: `.github/workflows/build.yml` — add Linux GUI build, add release asset upload for TUI/agent binaries, use release notes file where supported by `tauri-action`.
- Create: `scripts/check-version-consistency.mjs` — verify core version fields and obvious release doc references match `package.json`.
- Modify: `package.json` — add `check:release-version` script.
- Create: `LICENSE` — MIT license matching workspace metadata.

## Task 1: Fix headless/systemd documentation drift

**Files:**
- Modify: `packaging/systemd/README.md`
- Modify: `packaging/systemd/llm-relay-agent.service`

- [ ] **Step 1: Update `packaging/systemd/README.md`**

Replace the stale Keystore section with:

```md
## Keystore

Headless deployments must provide `LLM_RELAY_MASTER_KEY`, a base64-encoded
32-byte key. The agent uses it with AES-256-GCM and stores ciphertext at
`~/.llm-relay/secrets.env.enc` by default, or under `LLM_RELAY_RUNTIME_DIR` if
that variable is set.

Generate one once and keep it in your server's secret manager:

```sh
openssl rand -base64 32
```

For systemd user services, create an environment file readable only by your user:

```sh
mkdir -p ~/.config/llm-relay
chmod 700 ~/.config/llm-relay
printf 'LLM_RELAY_MASTER_KEY=%s\n' '<paste generated key>' > ~/.config/llm-relay/agent.env
chmod 600 ~/.config/llm-relay/agent.env
```

The bundled service file reads this file with `EnvironmentFile=%h/.config/llm-relay/agent.env`.
```

- [ ] **Step 2: Update `packaging/systemd/llm-relay-agent.service`**

Set documentation URL and add env file line:

```ini
[Unit]
Description=LLM Relay agent (headless gateway proxy)
Documentation=https://github.com/xuangong/llm-relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=LLM_RELAY_FOREGROUND=1
Environment=LLM_RELAY_RUNTIME_DIR=%h/.local/state/llm-relay
EnvironmentFile=%h/.config/llm-relay/agent.env
ExecStart=/usr/local/bin/llm-relay-agent
Restart=on-failure
RestartSec=5s

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.local/state/llm-relay %h/.local/share/llm-relay
PrivateTmp=true

[Install]
WantedBy=default.target
```

- [ ] **Step 3: Verify no stale passphrase docs remain**

Run: `rg "change-passphrase|passphrase|<org>" packaging README.md BUILD.md RELEASE_NOTES.md`

Expected: no output for stale passphrase or `<org>` references.

## Task 2: Fix public docs placeholders and release asset promises

**Files:**
- Modify: `README.md`
- Modify: `BUILD.md`
- Modify: `RELEASE_NOTES.md`

- [ ] **Step 1: Replace clone placeholders**

Change all `git clone <your-repo-url>` in `README.md` and `BUILD.md` to:

```bash
git clone https://github.com/xuangong/llm-relay.git
```

- [ ] **Step 2: Update README download list**

In `README.md` install section, include explicit TUI/agent asset naming:

```md
- **纯 TUI 服务器部署**：下载 `llm-relay-agent-<platform>` + `llm-relay-tui-<platform>`，例如 `llm-relay-agent-x86_64-unknown-linux-gnu` 和 `llm-relay-tui-x86_64-unknown-linux-gnu`
```

- [ ] **Step 3: Add release smoke test checklist to `BUILD.md`**

Add a section before the release checklist:

```md
### 发布前 smoke test

- [ ] 首次启动窗口显示正常
- [ ] 添加 Gateway 成功，健康检查返回状态
- [ ] 点击 Use 后 Claude / Codex / Gemini 配置文件写入本地代理地址
- [ ] `127.0.0.1:18080` 代理可接收请求
- [ ] 托盘 Quit 能完全退出 GUI
- [ ] `llm-relay-agent` 能在 headless 环境启动
- [ ] `llm-relay-tui` 能连接 agent
- [ ] GUI 检测到 agent 时能一键接管
```

- [ ] **Step 4: Update Tauri docs link**

Change `https://tauri.app/v1/guides/` to `https://tauri.app/start/`.

- [ ] **Step 5: Update release notes download table**

Add rows for:

```md
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |
| TUI/agent Windows | `llm-relay-agent-x86_64-pc-windows-msvc.exe`, `llm-relay-tui-x86_64-pc-windows-msvc.exe` |
```

## Task 3: Add release version consistency check

**Files:**
- Create: `scripts/check-version-consistency.mjs`
- Modify: `package.json`

- [ ] **Step 1: Create script**

Create `scripts/check-version-consistency.mjs` with logic that:
- reads `package.json` version
- checks `Cargo.toml` workspace package version
- checks `src-tauri/Cargo.toml` package version
- checks `src-tauri/tauri.conf.json` version
- checks `README.md`, `BUILD.md`, and `RELEASE_NOTES.md` do not contain another `0.x.y` release version different from package version
- prints all mismatches and exits 1 if any mismatch exists

- [ ] **Step 2: Add package script**

Add to `package.json` scripts:

```json
"check:release-version": "node scripts/check-version-consistency.mjs"
```

- [ ] **Step 3: Verify script passes**

Run: `pnpm check:release-version`

Expected: exits 0 and prints `Release version references are consistent: 0.3.0`.

## Task 4: Improve release CI assets

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Add Linux GUI to matrix**

Add matrix entry:

```yaml
          - platform: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            label: Linux-x64
```

- [ ] **Step 2: Install Linux Tauri dependencies conditionally**

Add before build:

```yaml
      - name: Install Linux dependencies
        if: matrix.platform == 'ubuntu-22.04'
        run: |
          sudo apt update
          sudo apt install -y libwebkit2gtk-4.1-dev \
            build-essential curl wget file \
            libxdo-dev libssl-dev \
            libayatana-appindicator3-dev librsvg2-dev pkg-config
```

- [ ] **Step 3: Run release checks before build**

Add:

```yaml
      - name: Check release version consistency
        run: pnpm check:release-version
```

- [ ] **Step 4: Build TUI/agent release binaries**

Add after Tauri build:

```yaml
      - name: Build TUI and agent
        run: cargo build --release --target ${{ matrix.target }} -p llm-relay-agent -p llm-relay-tui
```

- [ ] **Step 5: Upload TUI/agent binaries to draft release**

Add OS-specific shell steps that copy binaries to target-named filenames and upload them with `gh release upload ${{ github.ref_name }} ... --clobber`. Use `.exe` names on Windows.

- [ ] **Step 6: Prefer release notes file**

If `tauri-action` supports it, replace the fixed release body with release notes file input. If unsupported, keep fixed body but leave TUI/agent upload steps independent.

## Task 5: Add LICENSE

**Files:**
- Create: `LICENSE`

- [ ] **Step 1: Add MIT license text**

Use the standard MIT license with copyright:

```text
MIT License

Copyright (c) 2026 xuangong

Permission is hereby granted, free of charge, to any person obtaining a copy
...
```

- [ ] **Step 2: Verify license metadata alignment**

Run: `rg "license = \"MIT\"|MIT License" Cargo.toml LICENSE`

Expected: workspace metadata and license file both show MIT.

## Task 6: Final verification

**Files:**
- No source changes expected unless verification exposes issues.

- [ ] **Step 1: Run frontend checks**

Run: `pnpm typecheck && pnpm build:renderer && pnpm check:release-version`

Expected: all commands pass.

- [ ] **Step 2: Run Rust tests**

Run: `cargo test --workspace`

Expected: all non-ignored tests pass.

- [ ] **Step 3: Run ignored lifecycle tests**

Run: `cargo test -p llm-relay-agent --test lifecycle_integration -- --ignored --test-threads=1`

Expected: ignored lifecycle integration tests pass.

- [ ] **Step 4: Run ignored mutual exclusion tests on non-Windows**

Run: `cargo test -p llm-relay-agent --test mutual_exclusion -- --ignored --test-threads=1`

Expected: mutual exclusion tests pass.

- [ ] **Step 5: Build current-platform release package**

Run on macOS: `pnpm tauri build --target aarch64-apple-darwin`

Expected: DMG/app bundle produced under `src-tauri/target/aarch64-apple-darwin/release/bundle/`.

- [ ] **Step 6: Update release notes test line if counts differ**

If test counts differ from `RELEASE_NOTES.md`, update the test summary with the observed result.
