# Systemd Service Deployment for Jev Sentinel

`jev-sentinel` can run as a system-wide daemon or a systemd user service. Choose one, provision its configuration and secrets first, and review the [deployment boundaries](../../docs/deployment.md). These templates do not add authentication or approve production exposure. Starting either service runs configured probes, Jev evaluation and alerts.

---

## Option A: System-Wide Service (`/etc/systemd/system/`)

### 1. Install Binary and Configuration
```bash
# 1. Copy release binary to system path
sudo cp target/release/jev-sentinel /usr/local/bin/

# 2. Create config directory
sudo mkdir -p /etc/jev-sentinel

# 3. Copy your sentinel configuration
sudo cp sentinel.yaml /etc/jev-sentinel/sentinel.yaml
sudo chmod 600 /etc/jev-sentinel/sentinel.yaml

# 4. Configure environment variables (API keys, bot tokens, proxies)
sudo cp contrib/systemd/jev-sentinel.env.example /etc/default/jev-sentinel
sudo chmod 600 /etc/default/jev-sentinel
sudo nano /etc/default/jev-sentinel
```

### 2. Install and Enable Service
```bash
# 1. Copy service file
sudo cp contrib/systemd/jev-sentinel.service /etc/systemd/system/

# 2. Reload systemd daemon
sudo systemctl daemon-reload

# 3. Enable and start jev-sentinel
sudo systemctl enable --now jev-sentinel.service

# 4. Check status and streaming logs
sudo systemctl status jev-sentinel.service
journalctl -u jev-sentinel.service -f
```

---

## Option B: User-Level Service (`systemctl --user`)

Use this option when the configured probes can run with the user's permissions and a systemd user manager is available.

### 1. Install Binary and Configuration
```bash
# 1. The supplied unit's ExecStart uses ~/.cargo/bin/jev-sentinel.
# Install there explicitly:
cargo install --locked --path . --root "$HOME/.cargo"
# Or copy an already built binary to that exact path:
mkdir -p ~/.cargo/bin
cp target/release/jev-sentinel ~/.cargo/bin/jev-sentinel

# 2. Create user config directory
mkdir -p ~/.config/jev-sentinel

# 3. Place configuration and environment file
cp sentinel.yaml ~/.config/jev-sentinel/sentinel.yaml
cp contrib/systemd/jev-sentinel.env.example ~/.config/jev-sentinel/env
chmod 700 ~/.config/jev-sentinel
chmod 600 ~/.config/jev-sentinel/sentinel.yaml ~/.config/jev-sentinel/env
nano ~/.config/jev-sentinel/env
```

### 2. Install and Start User Service
```bash
# 1. Copy user service unit
mkdir -p ~/.config/systemd/user
cp contrib/systemd/jev-sentinel-user.service ~/.config/systemd/user/jev-sentinel.service

# 2. Optional: enable lingering after deciding this service should outlive login.
# This changes user service lifetime and may require administrator authorization.
loginctl enable-linger "$USER"

# 3. Reload and start
systemctl --user daemon-reload
systemctl --user enable --now jev-sentinel.service

# 4. View logs
journalctl --user -u jev-sentinel.service -f
```

---

## Useful Operations

| Task | System Command | User Command |
|---|---|---|
| **Status** | `sudo systemctl status jev-sentinel` | `systemctl --user status jev-sentinel` |
| **Restart** | `sudo systemctl restart jev-sentinel` | `systemctl --user restart jev-sentinel` |
| **Stop** | `sudo systemctl stop jev-sentinel` | `systemctl --user stop jev-sentinel` |
| **Follow Logs** | `journalctl -u jev-sentinel -f` | `journalctl --user -u jev-sentinel -f` |
| **Single real collection/evaluation** | `/usr/local/bin/jev-sentinel check --config /etc/jev-sentinel/sentinel.yaml` | `~/.cargo/bin/jev-sentinel check --config ~/.config/jev-sentinel/sentinel.yaml` |

`check` is not a dry-run: it executes configured probes and calls Jev. The table's foreground commands do not automatically inherit systemd EnvironmentFile values; use the intended protected environment, and do not paste secrets into shell history. Read permissions for the system configuration may require the service account or an administrator.

Keep `web.listen` on `127.0.0.1:8088` unless an approved authenticated access path protects the service. The existing starter template explicitly contains `0.0.0.0:8088` with web disabled; review/change that bind before enabling web. For SSH/exec targets, verify service-account permissions, absolute command/key paths and pinned host keys before starting. Neither service unit provisions credentials or host keys. The built-in macOS/Nvidia collectors require strict host-key verification; review the same policy in arbitrary exec commands.

## Deploying an updated dashboard

Static assets and locales are embedded in the Rust binary. Replacing source files does not update a running service. Build and verify the intended source snapshot, copy only its binary to the chosen unit's exact path, obtain authorization for that service restart, then inspect `systemctl status` and the actual dashboard/API through the approved access path. Restarting DSH is unrelated and must not be used to apply a Sentinel update.
