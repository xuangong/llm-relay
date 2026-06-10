# WSL2 Integration — Follow-up Items

> Follow-ups discovered during 2026-06-10/11 end-to-end verification of the
> stable-hostname design (`2026-06-10-wsl2-stable-hostname-design.md`).
> Each item is independently shippable; ordered by user-visible impact.

## 1. State machine should not inject `/etc/hosts` when no gateway is active

**Today.** The WSL state machine ticks every 60s, runs `resolve_url_for_distro`
for every selected distro, and writes `/etc/hosts` in tier 3 regardless of
whether a gateway is configured. After `Disable Relay`, the next tick
silently re-injects the hostname, even though there is nothing for it to
point at.

**Why it matters.** Two visible failures:

- A user disables relay, restarts WSL, runs `claude` from the distro — the
  CLI config still points at `llm-relay-<port>.host` (we don't proactively
  rewrite distro CLI configs on disable, see #4), and the hostname resolves
  to a dead gateway IP. Connect succeeds, but the relay isn't there.
- The defensive sweep in `service.clear_active` (e215248) runs once on
  disable, but the next state-machine tick re-injects. So clean-up is racy
  on the order of seconds.

**Decision.** Gate resolve on "active gateway exists." If `active_config.gateway_id`
is None, skip resolve entirely — neither probe nor inject. State machine still
ticks (it also reconciles distros + rebinds the listener on gateway IP change),
just without the URL-resolution side effect.

**Edge case.** When the user re-enables a gateway, the next tick (within 60s)
or the explicit `request_refresh` triggered by `apply_config` immediately
resolves and re-injects. No regression from today's UX.

**Where.** `crates/llm-relay-core/src/wsl/state.rs::StateMachine::tick`, the
"3. Re-probe URL for each selected distro" block. Read `active_config` once
at the top of the tick; if `gateway_id` is None, skip the loop.

## 2. Auto-switch must run apply, not just rewrite the DB row

**Today.** `crates/llm-relay-core/src/health.rs` auto-switch picks a healthy
gateway and writes the choice straight to `active_config` via
`db.set_active_config`. It never calls `service.set_active`. So the DB says
"active = X" while the on-disk CLI configs still point at the previously
applied gateway (or are absent entirely if the previous state was disabled).

**Why it matters.** After disable + auto-switch, every observable signal in
the GUI (tray, header, settings) shows the gateway as active and healthy.
The user thinks they can `claude "hi"` — but no apply has run since disable,
so the CLI config files have nothing valid in them. Manifests as silent
"connection refused" or stale-config bugs.

This was the root cause of one of the verification surprises on 2026-06-10:
right after a Disable click, the heartbeat log said `Auto-switch: none ->
Xian Zhang (first healthy)` and active_config got the gateway_id back, yet
no apply ran.

**Decision.** Auto-switch should call `service.set_active(gateway_id, key_id,
models)` so the full apply pipeline (`build_apply_targets` →
`apply_to_targets` → snapshot capture → hosts injection if needed) runs as
one atomic operation. If no key_id is available (e.g. first-ever auto-switch
after a fresh install with only one cached gateway), auto-switch should
**not** mutate active_config — log a warning and leave it.

**Where.** `crates/llm-relay-core/src/health.rs`, the auto-switch path
around line 175-190. Replace the direct DB write with an async call into
`service.set_active`. Threading: health loop already holds an `Arc<Service>`,
so this is a straightforward `.await`.

**Risk.** Auto-switch now does network work (fetch_keys against the gateway)
inside the health tick. Acceptable — the existing manual switch path already
does this, and a slow/failed fetch would just leave the previous active in
place, no worse than today's "rewrites DB silently" failure mode.

## 3. Surface a user-actionable error when Apply is missing a key

**Today.** `commands::apply_config` rejects with the literal string
`"key_id is required (no existing key to re-use)"`. The user sees a red
toast with that message and no hint what to do.

The actual fix is "click Login on the gateway row." The error doesn't say
that, and there's no contextual link.

**Why it matters.** Encountered twice in one verification session. Each
time we paused and figured it out from memory of the code.

**Decision.** Two-part:

1. **Error message.** Replace the string with something like
   `"This gateway has no API key. Click Login on the gateway row to fetch
   keys, then try again."` — direct, names the button, no jargon.
2. **GUI affordance** (stretch). If `apply_config` returns this specific
   error, the GUI could auto-highlight the Login button or pop a hint
   directly on the gateway row. Defer to part 2 until we see how often the
   string-only message actually fixes the issue in practice.

**Where.** `src-tauri/src/commands.rs::apply_config` line ~242. Pure string
change. No backend logic to touch.

## 4. Migrate DisableRelayDialog off the legacy single-file snapshot

**Today.** `commands::get_config_snapshot` reads
`~/.llm-relay/cli-config-backup.json` (the legacy single-file format) via
`config_writer::read_snapshot`. The `DisableRelayDialog` React component
uses this to show "what will be restored." But apply has not written this
file since the migration to `cli-config-backup/<target>.json` — it only
exists if `migrate_legacy_if_needed` found a pre-WSL-era file on first boot,
and gets deleted after the first apply.

So on any installation that has applied at least once since the WSL2
migration, the dialog shows "no snapshot" — completely failing to convey
what disable would actually do (restore the Windows snapshot at
`cli-config-backup/windows.json` plus zero or more
`cli-config-backup/wsl-<sha>.json`).

**Why it matters.** UX gap, not data loss — disable itself works because
`clear_targets_from_snapshots` walks the new per-target directory. The
dialog just doesn't *show* anything useful. Users may second-guess clicking
disable.

**Decision.** Replace `get_config_snapshot` with a new command that walks
`cli_config_backup_dir()` and returns one entry per snapshot
(`{kind: "windows" | "wsl", distro?: string, capturedAt: string,
fields: {...}}`). Update the dialog to render N rows instead of one.

Once `get_config_snapshot` has no callers, remove it AND the legacy
single-file machinery still hanging around with `#[allow(dead_code)]`:
`capture_*_snapshot`, `restore_*`, `delete_snapshot`,
`capture_snapshot_if_absent`, `read_snapshot`, `CliConfigSnapshot` struct.

**Where.**
- New command: `src-tauri/src/commands.rs` — e.g. `list_target_snapshots`.
- Snapshot iterator: `crates/llm-relay-core/src/config_writer/snapshot.rs`
  (mirror of `build_index` but returning richer rows).
- Frontend types: `src/lib/api.ts`.
- Dialog: `src/components/DisableRelayDialog.tsx` — render array.

**Risk.** Cosmetic. If the new command returns the wrong shape, dialog
breaks but disable itself still works.

## 5. Remove the dead legacy snapshot code

**Today.** Eight `#[allow(dead_code)]` annotations decorate the legacy
snapshot helpers in `config_writer/mod.rs` (commit adf6740 added them when
removing `apply_all_configs` / `clear_all_configs`). They're kept alive
solely because `get_config_snapshot` is still wired to `read_snapshot`.

Once #4 lands and the dialog is migrated, this code becomes purely dead.

**Decision.** Rip out (in this order, so each commit compiles):

1. `clear_all_configs`, `apply_all_configs` (already gone — adf6740).
2. `read_snapshot`, `get_config_snapshot` (#4).
3. `capture_claude_snapshot`, `capture_codex_snapshot`,
   `capture_gemini_snapshot`, `capture_snapshot_if_absent`.
4. `restore_claude`, `restore_codex`, `restore_gemini`, `delete_snapshot`.
5. `CliConfigSnapshot`, `ClaudeSnapshot`, `CodexSnapshot`, `GeminiSnapshot`
   (the old per-CLI snapshot structs — the new `snapshot::ClaudeSnapshot`
   etc. in `config_writer/snapshot.rs` are unrelated).
6. The `snapshot_path()` function that points to the legacy single-file path.
7. Migration: `snapshot::migrate_legacy_if_needed` stays — it's a one-shot
   that runs at startup and handles pre-WSL2 installs upgrading to v0.3.x.
   Keep it for at least one more release cycle, then drop in a follow-up.

**Where.** `crates/llm-relay-core/src/config_writer/mod.rs`.

**Risk.** Low — the dead-code lint already confirmed nothing internal
calls these. Test coverage stays clean because they had no tests of their
own.

## Order of operations

Suggested sequencing — each step is independent except where noted:

1. **#1** (state machine gating) — small, fixes a real silent bug.
2. **#2** (auto-switch via service) — medium, fixes a different silent bug
   that compounds with #1.
3. **#3** (error message) — trivial, ship anytime.
4. **#4** (dialog migration) — small frontend change + small backend
   command; blocks #5.
5. **#5** (delete dead code) — pure cleanup after #4.

#1 and #2 together close the "active state vs. observable state" gap that
caused most of the verification surprises. Worth treating them as a pair.

## Out of scope

Things explicitly deferred from this round:

- **Mirrored networking auto-detection.** When a distro reports mirror-mode
  (eth0 IP shared with host), tier 1 always wins, and tier 3 hosts injection
  is unnecessary. We could detect this and short-circuit, saving one
  `wsl.exe` invocation per probe. Not worth complicating the resolve
  orchestrator until users actually hit performance issues.
- **Multi-instance lifecycle helpers.** Today running real GUI (18080) + dev
  exe (18081) leaves two hosts entries with different port suffixes.
  Cleanup is independent per-instance, which is correct. No issue surfaced.
- **CLI config restoration without a snapshot.** Considered for the
  defensive sweep in #1 (e215248): if no snapshot exists, we have no
  reliable way to know the user's pre-relay `ANTHROPIC_BASE_URL` etc.
  Heuristics (revert to `localhost:4141`? to empty?) would do harm. Leave
  CLI configs alone in the snapshot-less case; only clean `/etc/hosts`
  (which carries an explicit `llm-relay-<port>.host` marker we own).
