# Cross-Platform Release Matrix Design

## Goal

Align the release pipeline and public documentation with the intended supported release targets:

- GUI app: Windows x64 and macOS Universal.
- TUI + headless agent: Linux x64, macOS Apple Silicon, and macOS Intel.

## Current State

The current release workflow uses one matrix for all artifacts. It builds the Tauri GUI for macOS Apple Silicon, macOS Intel, Windows x64, and Linux x64. It also builds and uploads TUI + agent binaries for every matrix entry, including Windows.

That produces assets outside the intended support matrix: Linux GUI packages, separate macOS GUI DMGs, and Windows TUI/agent binaries. README, BUILD.md, and RELEASE_NOTES.md also still describe those assets.

## Proposed Architecture

Split `.github/workflows/build.yml` into two release jobs with separate matrices.

### GUI release job

The GUI job builds only installer-style desktop assets:

| Platform | Target |
| --- | --- |
| macOS Universal | `universal-apple-darwin` |
| Windows x64 | `x86_64-pc-windows-msvc` |

The macOS job must install both macOS Rust targets before invoking Tauri because `universal-apple-darwin` needs both `aarch64-apple-darwin` and `x86_64-apple-darwin`. The Windows job installs only `x86_64-pc-windows-msvc`.

The job continues using `tauri-apps/tauri-action@v0` to create or update the draft release.

### TUI + agent release job

The CLI job builds and uploads only server/terminal assets:

| Platform | Target |
| --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |

Each job runs:

```bash
cargo build --release --target <target> -p llm-relay-agent -p llm-relay-tui
```

Then it copies the binaries into target-named release assets:

```text
llm-relay-agent-<target>
llm-relay-tui-<target>
```

and uploads them with:

```bash
gh release upload $GITHUB_REF_NAME release-assets/* --clobber
```

There is no Windows TUI/agent release asset. `.github/workflows/tui-ci.yml` may continue testing Windows builds to prevent regressions, but release support is limited to Linux and macOS.

## Public Documentation

Update release-facing docs so users see only supported release assets:

- `README.md` download section:
  - macOS: Universal DMG.
  - Windows: x64 installer.
  - TUI/headless: Linux x64 and macOS arm64/x64 binaries.
  - Remove Linux GUI and Windows TUI/agent release promises.
- `BUILD.md`:
  - Describe the official release matrix as GUI = macOS Universal + Windows x64, TUI/agent = Linux x64 + macOS arm64/x64.
  - Keep Linux GUI build instructions as source-build guidance only, not as a release artifact promise.
  - Update manual release checklist and upload examples to match the matrix.
- `RELEASE_NOTES.md`:
  - Replace macOS split GUI rows with macOS Universal.
  - Remove Linux GUI rows.
  - Remove Windows TUI/agent row.
  - Keep Linux and macOS TUI/agent rows.

## Verification

After implementation, run:

```bash
pnpm check:release-version
pnpm typecheck
pnpm build:renderer
cargo test --workspace
pnpm tauri build --target universal-apple-darwin
```

For release-target CLI builds, verify at least the current host target locally and rely on GitHub Actions for the full Linux/macOS target matrix. On macOS, the local CLI verification can include:

```bash
cargo build --release --target aarch64-apple-darwin -p llm-relay-agent -p llm-relay-tui
```

## Out of Scope

- Signing or notarization changes.
- Adding new supported architectures such as Linux arm64.
- Removing Windows TUI code or tests.
- Changing runtime behavior of GUI, TUI, or agent.
