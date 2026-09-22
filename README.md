# Jev Sentinel (`jev-sentinel`)

> Infrastructure observations, typed Jev advice, and Telegram alerts.

[![CI](https://github.com/PavelLizunov/jev-sentinel/actions/workflows/ci.yml/badge.svg)](https://github.com/PavelLizunov/jev-sentinel/actions/workflows/ci.yml)
[![Language: Rust](https://img.shields.io/badge/Language-Rust%202021-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Model: TypeSafe Jev](https://img.shields.io/badge/AI%20Model-TypeSafe%20Jev%20System%201-purple.svg)](https://typesafe.ai)

`jev-sentinel` is an infrastructure watchdog written in **Rust**. It collects telemetry across Linux, macOS, Proxmox and Nvidia probes, submits snapshots to **TypeSafe AI's Jev model**, and displays observations or dispatches Telegram alerts. Jev returns typed decisions; the client validates their schema and ranges, not the truth of a diagnosis. Self-healing currently logs suggested actions without executing commands.

---

## Scope

Sentinel separates observations from advisor output: a probe can still report metrics when Jev is unavailable, and missing advice is not interpreted as zero risk. It complements existing monitoring rather than replacing its inventory, persistent storage or alert management.

| Implemented | Not established by this revision |
|---|---|
| Bounded collection, explicit observation states and short in-memory history | Complete host/VM coverage or persistent history |
| Typed Jev response validation | Guaranteed diagnostic correctness or fixed inference latency/cost |
| Embedded dashboard with eight locales and three display modes | Full accessibility, cross-browser or native-language certification |
| Configured Telegram alerts and logging-only action suggestions | Automated remediation or production authentication |

There is no current release-size/RSS benchmark for this dashboard revision. Provider pricing and observed response times are not per-check guarantees.

---

## Key Features

- **Dependency-free dashboard assets:** Cards, Compact (default), and Table views, a fleet map, searchable categories, probe details, and native SVG history. HTML/CSS/JS are embedded in the binary; no npm, CDN, or new crates. HTTP connectivity, telemetry age, and Jev availability are displayed separately.
- **Extensible without Recompilation:** Anyone can add custom probes via the built-in `exec_probe` connector. Run any script (Bash, Python, Node, jq) that returns JSON, and Jev will evaluate it.
- **Specialized Collectors with Zombie Protection:**
  - Bounded subprocess runner with `kill_on_drop(true)`, 64 KiB buffer caps, and explicit process reaping to prevent leaks.
  - `macos_ssh`: Collects physical memory capacity, page counts, swap usage and configured process-presence checks. Missing metrics remain unknown; retained swap alone is not an exhaustion signal.
  - `nvidia_ssh`: Evaluates multi-GPU temperature, VRAM saturation, and utilization via `nvidia-smi`.
  - `http_probe`: Reusable connection pooling, status validation, and latency tracking.
  - `tcp_ping`: High-speed socket connection checks.
  - `exec_probe`: Arbitrary CLI commands and Proxmox `qm guest exec` checks.
- **Egress Proxy Support:** Routes external AI requests through corporate VPNs or local HTTP CONNECT proxies (`http://proxy.example.com:8080`).
- **Advisory self-healing:** Logs suggested actions. No service restart, cache purge, or operator-approval executor is implemented.

---

## Documentation

- [Documentation index](docs/README.md)
- [Cost optimization & LaTeX mathematical models](docs/cost-optimization.md)
- [Operator guide: search, map, metrics and freshness](docs/operator-guide.md)
- [Deployment, command side effects and security limits](docs/deployment.md)
- [Current state and next-chat handoff](docs/handoff-2026-09-20.md)

The original 29-probe Sentinel is restored at the owner-only Tailnet endpoint documented in the [handoff](docs/handoff-2026-09-20.md). Real category creation, key verification/reset, Jev classification and advancing observations passed through that HTTPS URL; the incorrect seven-system snapshot is retired. See the [publication acceptance](.dsh/publication-verification-2026-09-20.md). This is a running recovery checkpoint, not an autostart deployment or production-security certification.

## Quickstart

### 1. Build or Install

```bash
# Clone and build
git clone https://github.com/PavelLizunov/jev-sentinel.git
cd jev-sentinel
cargo build --release

# Inspect the built binary (linkage depends on the build target):
./target/release/jev-sentinel --help
```

### 2. Initialize Configuration

```bash
# Generate starter sentinel.yaml template
./target/release/jev-sentinel init
```

### 3. Run Instant Check

```bash
export TYPESAFE_API_KEY="your-typesafe-api-key"

# Single observation pass with full color table and Jev verdict:
./target/release/jev-sentinel check --config sentinel.yaml
```

Illustrative output only; these values are not a current infrastructure measurement:
```text
=========================================================
       Jev Sentinel — System 1 Infrastructure Check       
=========================================================
Loaded config: 6 targets configured

1. Collecting Multi-Node Telemetry...
TARGET               TYPE            STATUS       LATENCY   
------------------------------------------------------------
mac-mini-m4          macos_ssh       ONLINE          285.3ms
gpu-qwen-27b         http_probe      ONLINE          106.4ms
gpu-hardware-rtx     exec_probe      ONLINE         1402.5ms
hermes-dashboard     http_probe      ONLINE            6.6ms
caddy-ingress        tcp_ping        ONLINE            0.2ms
proxmox-pve1         exec_probe      ONLINE          403.5ms
------------------------------------------------------------
Collection finished in 1.40s (6/6 targets online)

2. Dispatching to TypeSafe Jev System 1 Model...

=========================================================
                 JEV SYSTEM 1 DECISION                   
=========================================================
• System Health:     HEALTHY
• Confidence:        95.0%
• Risk Score:        0.12 / 1.00
• Action Required:   NO
• Suggested Action:  none
• Jev Response Time: 1.234s
=========================================================
```

### 4. Verify Telegram Alerts

```bash
export TELEGRAM_BOT_TOKEN="your-bot-token"
export TELEGRAM_CHAT_ID="your-chat-id"

# Send an instant test alert to verify Telegram bot credentials and network/proxy route:
./target/release/jev-sentinel test-alert --config sentinel.yaml
```

### 5. Run Continuous Daemon

```bash
# Foreground daemon:
./target/release/jev-sentinel run --config sentinel.yaml

# Or enable the live Web Dashboard directly via CLI flags:
./target/release/jev-sentinel run --config sentinel.yaml --web --listen 127.0.0.1:8088
```
The default bind is `127.0.0.1:8088`. A non-loopback bind exposes the existing key/category mutation endpoints, which **do not provide application authentication**. Keep access behind a trusted authenticated gateway or local tunnel; this change does not deploy one.

#### Dashboard contract (API schema 2)

- `/`, `/dashboard.css`, `/dashboard.js`, `/i18n.js`, and eight whitelisted `/locales/<locale>.json` routes are compiled-in assets (GET/HEAD), not filesystem serving. Unknown locale paths return 404. `/api/status` includes `schema_version: 2`.
- **Eight dashboard languages:** English (`en`), Русский (`ru`), Deutsch (`de`), Français (`fr`), Español (`es`), Português (Brasil) (`pt-BR`), 简体中文 (`zh-CN`), 日本語 (`ja`). The header selector updates visible labels, dialogs, chart descriptions and accessible names without reloading or recollecting observations. Language is stored as `sentinel.locale`; blocked local storage does not stop monitoring.
- First use follows the browser language preference list, then English. Supported regional variants fall back to their base language; Portuguese variants use Brazilian Portuguese. Traditional Chinese (`zh-Hant`, Taiwan/Hong Kong/Macao) is not silently mapped to Simplified Chinese. Unknown browser languages fall through to the next preference or English. English is embedded as the per-key/per-form fallback. Failed or timed-out catalog loads retain the active language and show a message; a late response cannot overwrite a newer choice.
- Catalogs live in `src/web/locales/` with whole-message named parameters and native `Intl` number/date/plural formatting. Non-English catalogs are machine translations, **not certified by native speakers**; the interface shows this limitation. The initial catalogs received Gemini review; subsequent UX copy is covered by catalog/browser checks, not native-speaker review. No runtime translation service or external catalog fetch is used.
- Optional `error_code`, category `label_key`, and normalized `memory_basis_code` fields provide stable UI message mappings while retaining the original API text and schema version. User-created/edited category labels have no localization key. IDs, probe names/types, metric JSON, unknown diagnostic text and user-entered category data remain unchanged. Locale never changes sorting numbers, sample values or timestamps.
- A probe ID is `<type>:<unique configured name>`, independent of list ordering and categories. A probe is not an inferred VM or host. Renaming it changes identity. Names must be nonempty, unique, and at most 128 bytes; configuration allows at most 1024 probes.
- `daemon_epoch`, `view_revision`, and per-probe `sample_seq` identify updates. The latter two are decimal strings for JavaScript precision. History resets at daemon restart; it is not persisted to disk.
- Each probe has a fixed 128-sample numeric ring; the API sends the latest 20 samples. Repeated HTTP polls and Jev phase updates do not create observations. The 40-byte sample payload occupies 512,000 bytes for 100 full rings, excluding maps, snapshots, JSON and allocator overhead. This is arithmetic, not a measured RSS guarantee.
- Raw probe `status` is separate from `state`: `healthy`, `degraded`, `failed`, `unknown`, or `stale`. Transport timeout/unreachable means **unknown service state**, not a confirmed outage. An unexpected HTTP server status may be a confirmed failed probe.
- Freshness uses monotonic collector time. The age limit is `max(30 seconds, 2 × interval_seconds)`. `/api/status` recalculates `age_ms` on every request; the browser continues aging it between requests. A successful HTTP response does not make old telemetry fresh.
- Missing advice is `null`, never risk zero. Missing/wrong-type answers, out-of-range scores/probabilities, or unrequested choices invalidate the Jev result. Optional confidence remains `null` when absent. Schema validity is not evidence that a diagnosis is correct.
- History has real-time X coordinates and explicit gaps. RAM uses 0–100%; latency and swap use a zero-based labeled scale (SVG accessible label/tooltip). Details show all three charts. No forecast or RCA is implemented.
- `exec_probe` requires a complete JSON object or array; malformed/truncated output is `unknown` even with exit code zero. Numeric fields supported by normalization: `cpu_pct`, `ram_used_pct`, `memory_available_mib`, `memory_used_mib`, `memory_total_mib`, `swap_used_mib`; alternatively Proxmox object fields `cpu` (fraction), `mem`/`maxmem` (bytes). Proxmox arrays remain explicit resource rows in Details; no substring matching associates them with another probe.
- macOS capacity comes from `hw.memsize`. Available RAM is explicitly labeled a `free + inactive` estimate, not a memory-pressure measurement. Missing parses remain unknown. Allocated swap/free swap-pool size alone no longer marks macOS degraded.
- Public metric details are bounded/allowlisted. Raw stdout/stderr, arbitrary error bodies and URLs are not published in the dashboard. Keep sensitive values out of configured names and metric labels as well.
- Cards, Compact and Table share case-insensitive substring search across name, exact ID, type and category (ID or localized label). Active category/state restrictions are shown next to search. When restrictions hide matching probes, **Show matches across all probes** clears category and Issues only without clearing the query. **Clear filters** also resets grouping. View/filter preferences stay in browser local storage; API keys do not.
- The probe map starts expanded on desktop and mobile, with explicit **Hide map / Show map** controls. It always represents all probes, not only search results. The map has a bounded, keyboard-focusable scrolling region. Results use normal page scrolling and retain at least one viewport of height below the controls, including empty results, so filtering does not clamp page scroll and move the search field. Background polls do not programmatically reset page scroll.
- Cards and Compact show CPU, RAM and used Swap vertically without a repeated RAM value. Cards retain RAM/Swap history; Compact keeps the current values. Used Swap is shown in MiB in all views, including the mobile table: missing is `—`, measured zero is `0 MiB`, and no percentage is inferred without a total. Latency remains visible in the mobile table; CPU, RAM and age columns remain desktop-only.
- The UI key override affects **manual categorization only**; daemon evaluation retains its configured key.
- Self-healing remains logging-only. There is no restart/approve endpoint, adaptive scheduler, causal grouping, or alert gate in this iteration.

Validation without live credentials:

```bash
cargo test --locked
cargo fmt -- --check
# Native catalog/runtime tests: Node 22+, no browser needed.
node --test tests/dashboard-i18n.mjs
# Optional browser regression suite: Node 22+ and an already installed Chromium.
# No package installation or npm dependencies required.
CHROME_BIN=/path/to/chrome node tests/dashboard-browser.mjs
```

The browser suite uses a temporary loopback fixture server and an isolated browser profile, then closes both. It checks 100-probe views, filtering/sorting, keyboard focus, dialogs, failure states, stale data, live language switching and all eight locales at desktop and 320/390 px mobile widths. The native suite checks catalog keys/placeholders/plural forms, browser-language negotiation, unavailable storage, fallbacks and response races. These checks do not exercise live infrastructure, certify native-language quality or prove a two-second operator response time.

### 6. Background execution with systemd

Choose a system-wide or user service using the complete [systemd setup instructions](contrib/systemd/README.md), including binary/configuration paths, protected environment files and `daemon-reload`. Starting a service immediately enables configured collection, inference and alerts. The templates do not install authentication or resolve the [deployment blockers](docs/deployment.md#access-boundary).

---

## Configuration (`sentinel.yaml`)

```yaml
version: "1"

jev:
  api_key: "${TYPESAFE_API_KEY}"
  model: "jev-latest"
  base_url: "https://api.typesafe.ai"
  egress_proxy: "http://10.0.0.2:8080" # Optional proxy
  timeout_seconds: 15

daemon:
  interval_seconds: 60
  concurrency: 8

# Optional embedded web monitoring dashboard (zero new dependencies)
web:
  enabled: true
  listen: "127.0.0.1:8088" # Put authenticated access in front of any remote exposure

alerting:
  telegram:
    bot_token: "${TELEGRAM_BOT_TOKEN}"
    chat_id: "${TELEGRAM_CHAT_ID}"
    min_severity: "warning" # info, warning, critical
    # Optional proxy (defaults to jev.egress_proxy or TELEGRAM_PROXY env var):
    proxy: "${TELEGRAM_PROXY:-http://10.0.0.2:8080}"

self_healing:
  enabled: false # Logging-only today; there is no command executor
  allowed_services:
    - "core-service"
    - "caddy"
  require_confirmation: true # Not an implemented approval workflow

targets:
  - type: macos_ssh
    name: "mac-host"
    host: "10.0.0.50"
    user: "admin"
    ssh_key: "~/.ssh/id_ed25519"
    check_services: ["core-service"]

  - type: nvidia_ssh
    name: "gpu-node"
    host: "10.0.0.60"
    user: "admin"

  - type: http_probe
    name: "my-api"
    url: "https://api.my-domain.com/health"
    expected_status: 200

  - type: tcp_ping
    name: "db-port"
    host: "10.0.0.15"
    port: 5432

  - type: exec_probe
    name: "custom-sensor"
    command: ["python3", "/opt/sensors/check_power.py"]
```

---

## Multi-Platform Binaries

The release workflow is configured to build these targets for `v*` tags; publication and each platform's runtime behavior were not verified in this handoff:

- **Linux x86_64 GNU & musl** (`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`); GNU linkage is not guaranteed static
- **macOS Apple Silicon M1–M4** (`aarch64-apple-darwin`)
- **macOS Intel x86_64** (`x86_64-apple-darwin`)

The workflow packages the binary, README, license and template configuration, with SHA256 checksums as release assets.

---

## Operational boundaries

- **Footprint:** This dashboard/history revision has not been release-size or RSS-benchmarked. Earlier binary/RSS observations are not guarantees for a changed build. Numeric history is bounded; see the API contract above.
- **Subprocess Isolation:** All probe executions use bounded async runners with `kill_on_drop(true)`, 64 KiB buffer limits, and explicit process reaping on timeout.
- **Network Boundaries:** Configure `jev.egress_proxy` for the required AI gateway (`http://proxy.example.com:8080`). This revision did not exercise external services or audit gateway policy. SSH host-key verification and web authentication retain their existing limitations; use trusted access only.

---

## License

MIT License. Open source and free for personal and commercial homelab / enterprise use.
