# Cross-Platform Release Matrix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align release CI and public docs with the supported matrix: GUI app for Windows x64 and macOS Universal; TUI + headless agent for Linux x64 and macOS arm64/x64.

**Architecture:** Split release CI into a GUI job and a CLI job so each artifact family has its own target matrix. Keep Windows TUI CI tests for regression coverage, but stop publishing Windows TUI/agent binaries. Update README, BUILD.md, and RELEASE_NOTES.md so documented release assets exactly match CI output.

**Tech Stack:** GitHub Actions, Tauri 2, pnpm, Rust/Cargo, GitHub CLI release uploads, Markdown docs.

---

## File Map

- Modify: `.github/workflows/build.yml` — replace the single all-platform release matrix with separate `gui` and `cli` jobs.
- Modify: `README.md` — update prebuilt download guidance to macOS Universal + Windows GUI and Linux/macOS TUI/agent.
- Modify: `BUILD.md` — update official release matrix, manual upload examples, checklist, and keep Linux GUI as source-build-only guidance.
- Modify: `RELEASE_NOTES.md` — update test note and download table to match supported release assets.

## Task 1: Split release workflow by artifact family

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Replace `.github/workflows/build.yml` with separate GUI and CLI jobs**

Replace the full file with:

```yaml
name: Build & Release

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

permissions:
  contents: write

jobs:
  gui:
    strategy:
      fail-fast: false
      matrix:
        include:
          - platform: macos-latest
            target: universal-apple-darwin
            rustTargets: aarch64-apple-darwin,x86_64-apple-darwin
            label: macOS-universal
          - platform: windows-latest
            target: x86_64-pc-windows-msvc
            rustTargets: x86_64-pc-windows-msvc
            label: Windows-x64

    runs-on: ${{ matrix.platform }}
    name: Build GUI (${{ matrix.label }})

    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install pnpm
        run: npm install -g pnpm

      - name: Install frontend dependencies
        run: pnpm install --frozen-lockfile

      - name: Check release version consistency
        run: pnpm check:release-version

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.rustTargets }}

      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            src-tauri -> src-tauri/target

      - name: Build Tauri app
        uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          tagName: ${{ github.ref_name }}
          releaseName: 'LLM Relay ${{ github.ref_name }}'
          releaseBody: 'See RELEASE_NOTES.md and the assets to download and install this version.'
          releaseDraft: true
          prerelease: false
          args: --target ${{ matrix.target }}

  cli:
    strategy:
      fail-fast: false
      matrix:
        include:
          - platform: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            label: Linux-x64
          - platform: macos-latest
            target: aarch64-apple-darwin
            label: macOS-arm64
          - platform: macos-latest
            target: x86_64-apple-darwin
            label: macOS-x64

    runs-on: ${{ matrix.platform }}
    name: Build TUI/agent (${{ matrix.label }})

    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install pnpm
        run: npm install -g pnpm

      - name: Install frontend dependencies
        run: pnpm install --frozen-lockfile

      - name: Check release version consistency
        run: pnpm check:release-version

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            src-tauri -> src-tauri/target

      - name: Build TUI and agent
        run: cargo build --release --target ${{ matrix.target }} -p llm-relay-agent -p llm-relay-tui

      - name: Prepare TUI and agent assets
        run: |
          mkdir -p release-assets
          cp "target/${{ matrix.target }}/release/llm-relay-agent" "release-assets/llm-relay-agent-${{ matrix.target }}"
          cp "target/${{ matrix.target }}/release/llm-relay-tui" "release-assets/llm-relay-tui-${{ matrix.target }}"

      - name: Upload TUI and agent assets
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: gh release upload ${{ github.ref_name }} release-assets/* --clobber
```

- [ ] **Step 2: Verify workflow no longer publishes out-of-scope release targets**

Run:

```bash
rg "ubuntu-22.04|x86_64-unknown-linux-gnu|x86_64-pc-windows-msvc|universal-apple-darwin|aarch64-apple-darwin|x86_64-apple-darwin|Prepare TUI and agent assets \(Windows\)|llm-relay-tui.*windows" .github/workflows/build.yml
```

Expected output should show:

```text
.github/workflows/build.yml:          - platform: macos-latest
.github/workflows/build.yml:            target: universal-apple-darwin
.github/workflows/build.yml:            rustTargets: aarch64-apple-darwin,x86_64-apple-darwin
.github/workflows/build.yml:          - platform: windows-latest
.github/workflows/build.yml:            target: x86_64-pc-windows-msvc
.github/workflows/build.yml:          - platform: ubuntu-22.04
.github/workflows/build.yml:            target: x86_64-unknown-linux-gnu
.github/workflows/build.yml:          - platform: macos-latest
.github/workflows/build.yml:            target: aarch64-apple-darwin
.github/workflows/build.yml:          - platform: macos-latest
.github/workflows/build.yml:            target: x86_64-apple-darwin
```

The expected output must not include `Prepare TUI and agent assets (Windows)`.

- [ ] **Step 3: Commit workflow change**

Run:

```bash
git add .github/workflows/build.yml
git commit -m "ci(release): split gui and tui release targets"
```

## Task 2: Update README release asset guidance

**Files:**
- Modify: `README.md:90-100`

- [ ] **Step 1: Replace the prebuilt download bullet list**

In `README.md`, replace the bullets under `### 下载预编译包` with:

```md
- **macOS GUI**: `LLM Relay_0.3.0_universal.dmg`（Universal，支持 Apple Silicon + Intel）
- **Windows GUI**: `LLM Relay_0.3.0_x64-setup.exe`（NSIS 安装器）
- **纯 TUI / 服务器部署**：按平台下载下面列出的 agent + TUI 二进制：
  - Linux x64: `llm-relay-agent-x86_64-unknown-linux-gnu` + `llm-relay-tui-x86_64-unknown-linux-gnu`
  - macOS Apple Silicon: `llm-relay-agent-aarch64-apple-darwin` + `llm-relay-tui-aarch64-apple-darwin`
  - macOS Intel: `llm-relay-agent-x86_64-apple-darwin` + `llm-relay-tui-x86_64-apple-darwin`
  - systemd 部署见 [packaging/systemd/](packaging/systemd/)
```

- [ ] **Step 2: Verify README no longer promises Linux GUI or Windows TUI release assets**

Run:

```bash
rg "Linux.*\\.deb|AppImage|TUI/agent Windows|x86_64-pc-windows-msvc|LLM Relay_0.3.0_aarch64|LLM Relay_0.3.0_x64.dmg|universal" README.md
```

Expected output should include the macOS Universal line and should not include Linux GUI package bullets, Windows TUI/agent assets, or split macOS GUI DMG names.

- [ ] **Step 3: Commit README change**

Run:

```bash
git add README.md
git commit -m "docs(readme): align downloads with release matrix"
```

## Task 3: Update BUILD release instructions

**Files:**
- Modify: `BUILD.md`

- [ ] **Step 1: Update the opening scope sentence**

Replace line 3 with:

```md
本文档说明如何构建 LLM Relay（v0.3.0+）。官方 release 发布 GUI：macOS Universal + Windows x64；发布 TUI/agent：Linux x64 + macOS Apple Silicon + macOS Intel。
```

- [ ] **Step 2: Update macOS artifact description for Universal release**

In the macOS build section, replace the artifact block at `BUILD.md:102-111` with:

```md
### 构建产物位置

官方 release 使用 Universal DMG，同时支持 Apple Silicon 和 Intel：

```text
src-tauri/target/universal-apple-darwin/release/bundle/
├── macos/
│   └── LLM Relay.app
└── dmg/
    └── LLM Relay_0.3.0_universal.dmg
```
```

- [ ] **Step 3: Update macOS architecture output note**

Replace `两个 DMG 会分别落在：` and the two following bullet lines with:

```md
Universal DMG 会落在：
- `src-tauri/target/universal-apple-darwin/release/bundle/dmg/LLM Relay_0.3.0_universal.dmg`

如需本地调试单架构包，也可以分别使用 `aarch64-apple-darwin` 或 `x86_64-apple-darwin` 目标构建；这些单架构 DMG 不是官方 release 资产。
```

- [ ] **Step 4: Mark Linux GUI build as source-build only**

Change the heading `## Linux 构建` to:

```md
## Linux GUI 源码构建（非官方 release 资产）
```

Then insert this paragraph immediately below it:

```md
Linux GUI 包可从源码构建用于自用或调试，但官方 release 暂不发布 Linux GUI `.deb` / `.rpm` / `.AppImage`。Linux 服务器场景请使用 TUI + 无头 agent release 二进制。
```

- [ ] **Step 5: Replace macOS manual release commands with Universal commands**

In `BUILD.md:358-388`, replace the macOS manual release section body with:

```md
在 Apple Silicon Mac 上构建 Universal DMG：

```bash
cd llm-relay

# 1. 确认 tag 已推（CI 会尝试跑，失败没关系）
git tag v0.3.0        # 已打过就跳过
git push origin v0.3.0

# 2. 本地构建 Universal GUI
rustup target add aarch64-apple-darwin x86_64-apple-darwin
pnpm install
pnpm tauri build --target universal-apple-darwin

# 3. 把 DMG 传到 release（release 不存在时会自动创建）
gh release create v0.3.0 \
  --title "v0.3.0" \
  --notes-file RELEASE_NOTES.md \
  --draft \
  "src-tauri/target/universal-apple-darwin/release/bundle/dmg/LLM Relay_0.3.0_universal.dmg"

# release 已存在时追加资产
gh release upload v0.3.0 \
  "src-tauri/target/universal-apple-darwin/release/bundle/dmg/LLM Relay_0.3.0_universal.dmg" \
  --clobber
```
```

- [ ] **Step 6: Replace the Linux manual release section with TUI/agent release commands**

Replace `### Linux（用 WSL2 或原生 Ubuntu）` through its command block with:

```md
### Linux TUI/agent（用 WSL2 或原生 Ubuntu）

```bash
cargo build --release --target x86_64-unknown-linux-gnu -p llm-relay-agent -p llm-relay-tui
mkdir -p release-assets
cp target/x86_64-unknown-linux-gnu/release/llm-relay-agent release-assets/llm-relay-agent-x86_64-unknown-linux-gnu
cp target/x86_64-unknown-linux-gnu/release/llm-relay-tui release-assets/llm-relay-tui-x86_64-unknown-linux-gnu

gh release upload v0.3.0 release-assets/* --clobber
```
```

- [ ] **Step 7: Add macOS TUI/agent upload commands after Windows GUI commands**

After the Windows local build/upload command block, insert:

```md
### macOS TUI/agent 本地构建 + 上传

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin -p llm-relay-agent -p llm-relay-tui
cargo build --release --target x86_64-apple-darwin -p llm-relay-agent -p llm-relay-tui
mkdir -p release-assets
cp target/aarch64-apple-darwin/release/llm-relay-agent release-assets/llm-relay-agent-aarch64-apple-darwin
cp target/aarch64-apple-darwin/release/llm-relay-tui release-assets/llm-relay-tui-aarch64-apple-darwin
cp target/x86_64-apple-darwin/release/llm-relay-agent release-assets/llm-relay-agent-x86_64-apple-darwin
cp target/x86_64-apple-darwin/release/llm-relay-tui release-assets/llm-relay-tui-x86_64-apple-darwin

gh release upload v0.3.0 release-assets/* --clobber
```
```

- [ ] **Step 8: Update release checklist asset items**

Replace the asset upload checklist entries with:

```md
- [ ] macOS Universal DMG 上传
- [ ] Windows NSIS exe 上传
- [ ] Linux x64 TUI/agent 二进制上传
- [ ] macOS Apple Silicon + Intel TUI/agent 二进制上传
```

- [ ] **Step 9: Verify BUILD docs contain the supported matrix and no stale release checklist promises**

Run:

```bash
rg "official release|官方 release|Universal|Linux GUI 源码构建|Linux deb|Linux deb \+ AppImage|TUI/agent Windows|macOS aarch64 DMG|macOS x64 DMG|x86_64-pc-windows-msvc" BUILD.md
```

Expected: output includes official release matrix, Universal references, and Linux GUI source-build-only wording. Output must not include `Linux deb + AppImage`, `TUI/agent Windows`, `macOS aarch64 DMG 上传`, or `macOS x64 DMG 上传`.

- [ ] **Step 10: Commit BUILD docs change**

Run:

```bash
git add BUILD.md
git commit -m "docs(build): document supported release targets"
```

## Task 4: Update release notes download table

**Files:**
- Modify: `RELEASE_NOTES.md:31-52`

- [ ] **Step 1: Update the macOS build verification line**

Replace:

```md
- `pnpm tauri build --target aarch64-apple-darwin` 成功生成 macOS Apple Silicon DMG
```

with:

```md
- `pnpm tauri build --target universal-apple-darwin` 成功生成 macOS Universal DMG
```

- [ ] **Step 2: Replace the download table rows**

Replace the rows under the `| 平台 | 文件 |` header with:

```md
| macOS Universal | `LLM Relay_0.3.0_universal.dmg` |
| Windows GUI | `LLM Relay_0.3.0_x64-setup.exe` |
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |
```

Keep the warning paragraph after the table.

- [ ] **Step 3: Verify release notes contain no stale release asset rows**

Run:

```bash
rg "aarch64.dmg|x64.dmg|Linux \(Debian|AppImage|TUI/agent Windows|x86_64-pc-windows-msvc|universal" RELEASE_NOTES.md
```

Expected: output includes the Universal DMG line and no split macOS GUI rows, Linux GUI rows, or Windows TUI/agent row.

- [ ] **Step 4: Commit release notes change**

Run:

```bash
git add RELEASE_NOTES.md
git commit -m "docs(release): update supported asset table"
```

## Task 5: Final verification

**Files:**
- No source changes expected unless verification exposes issues.

- [ ] **Step 1: Run version consistency check**

Run:

```bash
pnpm check:release-version
```

Expected:

```text
Release version references are consistent: 0.3.0
```

- [ ] **Step 2: Run frontend checks**

Run:

```bash
pnpm typecheck && pnpm build:renderer
```

Expected: both commands exit 0.

- [ ] **Step 3: Run Rust workspace tests**

Run:

```bash
cargo test --workspace
```

Expected: all non-ignored workspace tests pass.

- [ ] **Step 4: Verify macOS Universal GUI build on macOS**

Run:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
pnpm tauri build --target universal-apple-darwin
```

Expected: Tauri produces a Universal DMG under:

```text
src-tauri/target/universal-apple-darwin/release/bundle/dmg/
```

- [ ] **Step 5: Verify current-host TUI/agent release build**

On macOS, run:

```bash
cargo build --release --target aarch64-apple-darwin -p llm-relay-agent -p llm-relay-tui
```

Expected: both binaries build under:

```text
target/aarch64-apple-darwin/release/
```

- [ ] **Step 6: Verify stale release promises are removed from public docs and release workflow**

Run:

```bash
rg "TUI/agent Windows|llm-relay-tui-x86_64-pc-windows-msvc|llm-relay-agent-x86_64-pc-windows-msvc|LLM Relay_0.3.0_aarch64.dmg|LLM Relay_0.3.0_x64.dmg|Linux deb \+ AppImage|Linux \(Debian/Ubuntu\)|AppImage（通用）" README.md BUILD.md RELEASE_NOTES.md .github/workflows/build.yml
```

Expected: no output.

- [ ] **Step 7: Commit any verification-driven doc corrections**

If verification exposed doc drift, edit the affected file and run:

```bash
git add README.md BUILD.md RELEASE_NOTES.md .github/workflows/build.yml
git commit -m "docs(release): fix release matrix drift"
```

If no corrections were needed, skip this step.
