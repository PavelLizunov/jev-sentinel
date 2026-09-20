# Deployment and current limits

## Access boundary

The Rust dashboard has **no application authentication**. Existing POST routes can change categories, change the manual-categorization key override and request paid Jev inference. The response origin policy is permissive; the POST body lacks a whole-request deadline. These web findings remain unresolved: [original security report](../.dsh/security-review-report.md). The recovery patch changes `macos_ssh` and `nvidia_ssh` to `StrictHostKeyChecking=yes`; unknown or changed host keys fail closed. Arbitrary `exec_probe` SSH commands must also enforce the configured trust policy.

Keep the daemon on loopback behind a trusted authenticated gateway or a local tunnel. A private IP or Tailscale address narrows network reachability but is not application authorization. Do not expose it publicly. No gateway is installed by this repository's setup instructions.

```sh
./target/release/jev-sentinel run --config /absolute/path/sentinel.yaml --web --listen 127.0.0.1:8088
```

This command starts collection and evaluation; it is not a preview-only command.

## Current recovery publication

On20 September the owner activated the approved Tailnet HTTPS8099 route through an owner-scoped Caddy gateway on loopback18089. The exact HTTPS endpoint passed real inventory, category/key/Jev and advancing-cycle acceptance; an owner Debian Tailnet client also reached it. Existing DSH routes are unchanged. The [publication report](../.dsh/publication-verification-2026-09-20.md) records evidence and limitations. Sentinel and Caddy run as managed jobs without systemd/autostart; native backend security findings and the gateway's empty200 timeout-response caveat remain. This is a recovery checkpoint, not a general production deployment.

## CLI side effects

| Command | Effects |
|---|---|
| `init --path sentinel.yaml` | Writes a starter YAML if that path does not already exist |
| `check --config sentinel.yaml` | Runs configured HTTP/TCP/SSH/exec probes and sends a snapshot to the configured Jev provider; no Telegram unless alert flags request it |
| `check --alert` / `check --force-alert` | Can send Telegram alerts after evaluation |
| `test-alert --config sentinel.yaml` | Sends an actual Telegram test message |
| `run --config sentinel.yaml` | Repeats collection/evaluation and dispatches configured alerts; optionally serves the dashboard |

`check` is **not a dry-run or config validator**. `exec_probe` runs the configured command; reviewed configuration is a trust boundary. Self-healing currently logs suggestions only; `require_confirmation` is not an implemented operator approval workflow.

Do not place keys, tokens, full configuration or raw database copies in screenshots, Git or shareable reports. Dashboard metric allowlisting does not sanitize the separate collector-to-Jev request. The UI key override applies only to manual categorization; it does not replace the daemon evaluation key.

## Configuration

Start from [sentinel.example.yaml](../sentinel.example.yaml), review every target, then provision secrets in a protected environment file. `sentinel.homelab-full.yaml` contains the29 targets used by the owner's earlier Sentinel daemon; historical launch evidence is recorded in the [handoff](handoff-2026-09-20.md). The file alone does not establish a currently running service. It includes deployment-specific addresses, an unrestricted listener and `exec_probe` commands that return plain text instead of the current required JSON. Preserve the intended target identities during recovery; do not replace them with another monitor's smaller inventory.

- `${NAME}` uses an environment variable; `${NAME:-fallback}` supplies a fallback. Missing variables without a fallback become empty strings. Review values before starting.
- Code defaults: interval 60 seconds, concurrency 8, web disabled, web listener `127.0.0.1:8088`. The existing `sentinel.example.yaml` explicitly sets `0.0.0.0:8088` even though web is disabled. Before enabling it, change that value to loopback or use an approved protected access path; this documentation turn did not alter the template.
- Interval and concurrency must be positive. At most 1024 probes; nonempty unique names at most 128 bytes.
- `exec_probe` must emit a complete JSON object or array. Non-JSON or truncated output is Unknown even if exit status is zero.
- Configure the required Jev egress proxy in the homelab. Do not use missing credentials as a deliberate way to disable outbound evaluation; a preview should not launch the daemon.
- `~` in a configured SSH key path is not a promise of shell expansion. Use an absolute service-account path and pre-provision pinned host keys.
- The current CLI initializes logging at INFO. `daemon.log_level` and the environment template's `RUST_LOG` are not applied by that initialization; do not rely on them to suppress sensitive logs.

## Build, assets and services

```sh
cargo build --release --locked
./target/release/jev-sentinel --help
```

Rust 2021 is the source edition, not an MSRV declaration. The GNU build is not guaranteed static. The release workflow defines GNU/musl Linux and Intel/Apple Silicon macOS artifacts, but a workflow file does not prove that a release was published or that a given artifact was tested.

HTML/CSS/JS and locale catalogs are compiled into the binary. Editing `src/web` does not update an already running daemon. Build the intended snapshot, identify the exact installed binary/service, preserve configuration, obtain the required restart permission, then verify the actual consumer URL after deployment. Restarting DSH does not deploy Sentinel and can disrupt active work.

Use the [systemd instructions](../contrib/systemd/README.md) for exact paths. Choose one service type. The user unit expects `~/.cargo/bin/jev-sentinel`; the system unit expects `/usr/local/bin/jev-sentinel`. Start commands can run probes, paid inference and alerts immediately. Service templates are not a production security approval.

## Verification without live credentials

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt -- --check
node --test tests/dashboard-i18n.mjs
CHROME_BIN=/absolute/path/to/chrome node tests/dashboard-browser.mjs
```

Node 22+ and an already installed Chromium are needed for browser tests; no npm install or CDN assets are required. The suite uses isolated browser storage and a temporary loopback fixture server. On control-plane hosts, honor the local build guard and resource policy rather than installing toolchains or bypassing limits.

Verified counts and exact snapshots belong in the dated [verification reports](README.md#verification-and-research), not permanent compatibility promises. Live recovery acceptance is explicitly opt-in through `tests/dashboard-live.mjs`: it uses a real daemon, calls Jev, verifies a supplied key and creates a temporary category. Read its invocation and cleanup contract first; it is not an offline regression suite. The recovery record distinguishes verified local real-provider interactions from pending published-URL acceptance.

Known gaps: Firefox/Safari, physical devices/software keyboards, screen readers, native-language certification, independent review of the latest delta, Telegram acceptance, release-size/RSS measurement and human usability retest.

## Deferred, not implemented

Production auth/authorization/CSRF/body-deadline hardening; persistent history and category/key-override persistence across daemon restarts; continuous Beszel/Pulse adapter; VM/probe reconciliation; new collectors; automated remediation; causal incident grouping; adaptive schedules and performance guarantees. These require a separate decision. The approved recovery is not a full production-hardening release.
