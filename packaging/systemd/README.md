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
