# Running the LLM Relay agent under systemd

For headless Linux servers (no GUI), run the agent as a per-user systemd
service.

## Install

1. Build the agent and install the binary:
   ```sh
   cargo build --release -p llm-relay-agent
   sudo install -m 0755 target/release/llm-relay-agent /usr/local/bin/
   ```

2. Create the required master-key environment file:
   ```sh
   mkdir -p ~/.config/llm-relay
   chmod 700 ~/.config/llm-relay
   printf 'LLM_RELAY_MASTER_KEY=%s\n' '<paste generated key>' > ~/.config/llm-relay/agent.env
   chmod 600 ~/.config/llm-relay/agent.env
   ```

   Generate the key once with:
   ```sh
   openssl rand -base64 32
   ```

3. Drop the unit into your user systemd dir:
   ```sh
   mkdir -p ~/.config/systemd/user
   cp packaging/systemd/llm-relay-agent.service ~/.config/systemd/user/
   ```

4. Enable + start:
   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now llm-relay-agent.service
   ```

5. Linger so the agent runs without an active session:
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

Headless deployments must provide `LLM_RELAY_MASTER_KEY`, a base64-encoded
32-byte key. The agent uses it with AES-256-GCM and stores ciphertext at
`~/.llm-relay/secrets.env.enc`. The app database also lives under
`~/.llm-relay/`.

`LLM_RELAY_RUNTIME_DIR` controls runtime lock, PID, socket, and log paths; it
does not move the app database or env keystore.

The bundled service file reads the master key from
`EnvironmentFile=%h/.config/llm-relay/agent.env`. The service will fail to start
if that file is missing or unreadable.
