# Review: TUI Design Spec

- Reviewed spec: `docs/superpowers/specs/2026-04-20-tui-design.md`
- Review date: `2026-04-20`

## Findings

1. High: The IPC design cannot safely support both request/response traffic and unsolicited subscriptions as written.

   The spec defines `Subscribe` plus server-pushed `Event` messages, but `Response` has no request id or correlation id, only `Ok`, `Error`, `Snapshot`, and `Event`. Once the TUI both subscribes and issues normal RPCs on the same connection, it cannot reliably determine whether the next message is a response to an in-flight request or an unrelated event. This needs either request ids, separate command/event channels, or a protocol with an explicit envelope.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:99`
   `docs/superpowers/specs/2026-04-20-tui-design.md:114`

2. High: The mutual-exclusion story is not actually cross-platform even though Windows is listed as a target platform.

   The spec claims GUI and TUI are mutually exclusive via `flock(~/.llm-relay/agent.lock)` plus port checks. `flock` is Unix-specific, while the document explicitly lists Windows as a first-class target. Unless this is replaced with a cross-platform file lock or a named mutex abstraction, the core guarantee does not hold on Windows.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:5`
   `docs/superpowers/specs/2026-04-20-tui-design.md:67`
   `docs/superpowers/specs/2026-04-20-tui-design.md:327`

3. High: The device-login section mixes browser URLs and API endpoints, and it hard-codes a polling cadence that disagrees with the current implementation.

   The spec says to reuse `"/device/login"`, `startDeviceLogin`, and `pollDeviceLogin`, then says the agent polls every 5 seconds. In the current code, `startDeviceLogin` and `pollDeviceLogin` call `/auth/device/code` and `/auth/device/poll`, while `/device/login` is only the page the user opens in a browser. The API also returns an `interval` field, so baking in a fixed 5-second poll interval is likely wrong.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:219`
   `docs/superpowers/specs/2026-04-20-tui-design.md:245`
   `src-tauri/src/gateway.rs:145`
   `src-tauri/src/gateway.rs:160`
   `src-tauri/src/gateway.rs:180`
   `src/components/SignInDialog.tsx:71`

4. Medium: `SetActive` drops key identity that the current app persists and surfaces in the UI.

   The proposed RPC only carries `gateway_id`, a raw `key`, and model selection. The current data model stores `key_id`, `key_name`, and `key_value`, and the UI/tray surfaces the selected key name. If IPC reduces this to only a raw key string, the TUI cannot reliably restore which logical key was chosen after reconnects, and parity with the current UI state model is lost.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:103`
   `docs/superpowers/specs/2026-04-20-tui-design.md:143`
   `docs/superpowers/specs/2026-04-20-tui-design.md:154`
   `src-tauri/src/database.rs:45`
   `src-tauri/src/commands.rs:205`
   `src-tauri/src/tray.rs:68`

5. Medium: The wire-format description is internally inconsistent.

   The protocol is described as both `length-prefixed + JSON` and `one-line NDJSON`. Those are different framing strategies. If this is left unresolved, implementation and testing can both proceed against incompatible assumptions and still appear locally correct.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:93`
   `docs/superpowers/specs/2026-04-20-tui-design.md:297`

## Open Questions

1. The keystore fallback detection is based on `DISPLAY` and `DBUS_SESSION_BUS_ADDRESS`. Would it be safer to attempt the system keychain first and only fall back on real initialization errors, rather than environment heuristics?

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:264`
   `docs/superpowers/specs/2026-04-20-tui-design.md:289`

2. The test plan covers codec and spawn smoke tests, but it does not mention stale socket cleanup, PID reuse, or lock-race scenarios. Those lifecycle failures are the ones most likely to break attach/detach behavior in production.

   References:
   `docs/superpowers/specs/2026-04-20-tui-design.md:286`
   `docs/superpowers/specs/2026-04-20-tui-design.md:297`
   `docs/superpowers/specs/2026-04-20-tui-design.md:300`
