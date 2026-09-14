# Official login handoff by environment

Windows Host and each WSL distro have independent environment checkboxes.
An identifiable official Codex or Claude login unchecks only its environment,
releasing all managed clients in that environment. Other environments and global
client selections remain unchanged. If no environments remain selected, Relay
is disabled and both listeners stop before configuration restoration. A cleanup
failure must not keep those listeners running.

The GUI and agent check Windows and running WSL distros every three seconds.
Stopped distros are not started by this monitor. Apply and Disable also check
before changing configuration. A second check after gateway key lookup covers
logins during the network wait.

## Durable handoff

1. Persist the environment disable and transfer all its file records into a
   cleanup journal before restoration can run.
2. For clients that logged in, preserve official credentials and account state,
   remove Relay routing and placeholder credentials, and restore unchanged
   writer defaults by comparing applied snapshots. Preserve unrelated edits.
   Other clients in that environment follow ordinary original restoration.
3. Delete obsolete `.origin`, `.bak` and `.applied` files only after successful
   cleanup. Journal failures for retry and block re-enabling until cleanup ends.
4. Show the login explanation under Windows Host or the affected WSL distro.
   Explicitly reselecting the environment captures its current configuration as
   the new ground truth on apply. Global Use and gateway switching do not
   silently reselect disabled environments. If all are off, select an environment
   before Use.

No credential tokens or checksums are stored in lifecycle metadata. The two
`.applied` configuration sidecars contain writer output for field comparison,
not copies of OAuth credential files.

## Detection boundaries

- Codex: file-backed `auth.json` with ChatGPT access and refresh tokens, no API
  key, and `auth_mode` either omitted (older format) or `chatgpt`.
- Claude: a new account/organization, newly present OAuth credentials, or a new
  refresh-token lifetime. Access tokens, rotating refresh-token strings, access
  expiry, profile timestamps and ordinary settings edits are not login signals.
- Existing Claude credentials are baselined before takeover. On upgrading an
  already active installation without this baseline, the first observation is
  used as baseline; it cannot prove that a login happened while Relay was absent.
- Older Claude versions without a refresh-token lifetime cannot reliably expose
  a same-account re-login if the intervening logout was not observed. Secure
  store-only credentials, custom CLI home directories and inherited process or
  project-level environment overrides are not covered. Do not claim every
  `/login` invocation can be identified from ordinary file changes.
- This changes persisted configuration; it does not terminate CLI processes or
  alter an already-running process's inherited environment.

Claude's documented authentication precedence explains why removing Relay's
endpoint and placeholder token matters in addition to preserving login:
[Claude Code authentication](https://code.claude.com/docs/en/authentication).

Tests use isolated filesystem backends and actual local TCP listeners. They do
not run real OAuth flows or alter the user's Windows or WSL credentials.
