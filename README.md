# Jev Sentinel (`jev-sentinel`)

> **Autonomous System 1 Infrastructure Watchdog and Self-Healing Daemon powered by TypeSafe Jev.**

[![Language: Rust](https://img.shields.io/badge/Language-Rust%202021-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Binary Size](https://img.shields.io/badge/Binary-5.2%20MB-success.svg)]()
[![Model: TypeSafe Jev](https://img.shields.io/badge/AI%20Model-TypeSafe%20Jev%20System%201-purple.svg)](https://typesafe.ai)

`jev-sentinel` is a high-performance, compiled infrastructure watchdog written in **Rust**. It continuously collects multi-dimensional telemetry (RAM, swap growth, GPU VRAM/temperature, HTTP endpoints, systemd/launchd services) across heterogeneous nodes (Linux, macOS, Proxmox, Nvidia GPUs), submits the combined state to **TypeSafe AI's Jev model** (System 1 inference in ~150ms with 0% hallucinations), and executes safe, bounded self-healing actions or dispatches rich Telegram alerts.

---

## Why Jev Sentinel vs. Traditional Monitoring?

| Feature | Traditional Tools (Zabbix / Prometheus) | Heavy LLMs (GPT-4 / Claude) | **Jev Sentinel (System 1)** |
|---|---|---|:---:|
| **Evaluation Logic** | Rigid scalar thresholds (`if free < 500MB`) | Free-form natural language | **Multi-dimensional typed decisions (`Noul`, `Choice`, `Score`)** |
| **Hallucinations** | None (dumb thresholds) | Risk of schema/syntax errors | **0% Hallucinations (Mathematically constrained)** |
| **Inference Latency** | Instant | 3 – 15 seconds | **~150 ms** |
| **API Cost** | Free | $0.01 – $0.05 per check | **$0.00003 per check ($42 per billion tokens)** |
| **Daemon Footprint** | Varies | Heavy Python / Node runtime | **Single 5.2 MB binary, ~8 MB RAM** |

---

## Key Features

- **Extensible without Recompilation:** Anyone can add custom probes via the built-in `exec_probe` connector. Run any script (Bash, Python, Node, jq) that returns JSON, and Jev will evaluate it.
- **Specialized Collectors:**
  - `macos_ssh`: Evaluates Apple Silicon unified memory, wired pages, swap usage, and LaunchAgent processes. Prevents FileVault lockouts and kernel panics.
  - `nvidia_ssh`: Evaluates Nvidia GPU temperature, VRAM saturation, and utilization via `nvidia-smi`.
  - `http_probe`: Verifies API latency and status codes.
  - `tcp_ping`: High-speed socket connection checks.
  - `exec_probe`: Arbitrary CLI commands and Proxmox `qm guest exec` checks.
- **Egress Proxy Support:** Seamlessly routes external AI requests through corporate VPNs or local HTTP CONNECT proxies (`http://192.168.0.142:18080`).
- **Bounded Self-Healing:** Pre-approves service restarts (`launchctl`, `systemctl`) before OS-level crashes happen.

---

## Quickstart

### 1. Build or Install

```bash
# Clone and build
git clone https://github.com/PavelLizunov/jev-sentinel.git
cd jev-sentinel
cargo build --release

# The single stripped static binary is in:
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

Output example:
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

### 4. Run Continuous Daemon

```bash
./target/release/jev-sentinel run --config sentinel.yaml
```

---

## Configuration (`sentinel.yaml`)

```yaml
version: "1"

jev:
  api_key: "${TYPESAFE_API_KEY}"
  model: "jev-latest"
  base_url: "https://api.typesafe.ai"
  egress_proxy: "http://192.168.0.142:18080" # Optional proxy
  timeout_seconds: 15

daemon:
  interval_seconds: 60
  concurrency: 8

alerting:
  telegram:
    bot_token: "${TELEGRAM_BOT_TOKEN}"
    chat_id: "${TELEGRAM_CHAT_ID}"
    min_severity: "warning" # info, warning, critical

self_healing:
  enabled: true
  allowed_services:
    - "osaurus"
    - "caddy"
  require_confirmation: false

targets:
  - type: macos_ssh
    name: "mac-mini-m4"
    host: "100.116.97.112"
    user: "slovn"
    ssh_key: "~/.ssh/id_ed25519"
    check_services: ["osaurus", "gemma-4-e4b"]

  - type: nvidia_ssh
    name: "gpu-node"
    host: "x3d-pve-1"
    user: "root"

  - type: http_probe
    name: "my-api"
    url: "https://api.my-domain.com/health"
    expected_status: 200

  - type: tcp_ping
    name: "db-port"
    host: "192.168.0.210"
    port: 5432

  - type: exec_probe
    name: "custom-sensor"
    command: ["python3", "/opt/sensors/check_power.py"]
```

---

## License

MIT License. Open source and free for personal and commercial homelab / enterprise use.
