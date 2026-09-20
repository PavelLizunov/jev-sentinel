# Sentinel publication acceptance — 20 September 2026

## Verdict and scope

**Verified for the approved recovery contract:** original29 Sentinel probes and working category/key/Jev UI at **https://harness-test.tail9fd337.ts.net:8099/**. This is not a production-hardening certification. Owner approval at13:56 MSK covered separate Tailscale Serve + Caddy publication, existing routes unchanged, no DSH restart, no Telegram/remediation.

The initial privilege blocker was resolved by the owner running the route command in an ordinary administrative SSH session on harness-test. The agent did not bypass `no_new_privileges`. Comparing live Serve JSON against the saved baseline proved that only HTTPS8099→127.0.0.1:18089 was added;443/8443/3092 remained identical and Funnel absent. The owner's earlier accidental Omarchy route is separate and was not changed by the agent.

## Consumer evidence

Artifact root: `/var/lib/dsh/.dsh/task-artifacts/jev-publication-2026-09-20/` (private operational data, do not publish in public Git).

1. Genuine owner Debian Tailnet peer, pinned SSH, curl GET `/api/status`:29 targets, cycle46, original daemon epoch. No client-supplied identity headers, TLS certificate verification enabled.
2. Job`bash-141`, `LIVE_URL=https://harness-test.tail9fd337.ts.net:8099 LIVE_MUTATIONS=approved CHROME_BIN=/var/lib/dsh/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome ARTIFACT_DIR=<root>/browser node tests/dashboard-live.mjs`, approved key provided only on stdin: **exit0,8 groups**. Chromium itself ran on harness-test and reached the actual HTTPS Serve endpoint without identity spoofing or TLS bypass; the separate Debian check establishes off-host reachability.
   - Exact29 original IDs and embedded CSS/JS/Russian catalog hashes.
   -29 targets in Cards, Compact and Table; real action buttons enabled; no snapshot wrapper.
   -Category form creates server state and visible filter option.
   -Key form verifies real key, saves masked override, clears input, resets to server key.
   -Real Jev classification assigns29/29 targets.
   -Cycles47→48 advance timestamp/sample_seq for every target and preserve categories/assignments.
   -Desktop1440/mobile390 have no horizontal overflow; macOS details open.
   -Four action POST responses200; zero JS exceptions. Evidence: `browser/live-acceptance.json` and screenshots.
3. Through HTTPS from the genuine owner Debian peer: cross-origin POST403, missing-Origin POST403, preflight403, oversized POST413; no wildcard CORS response. None of these negative requests changes application state.
4. Cleanup: pre-ui-state and live definitions matched the original five categories plus only the test category; no custom key remained. Stopped only own Sentinel`bash-126`, launched`bash-142` from an exact frozen copy of the tested executable, then classified29/29 again through HTTPS. Five defaults, no test category/override. History necessarily restarted. Old incorrect snapshot`bash-115` stopped after real URL acceptance.

## Exact runtime checkpoint

Sentinel PID3137898, job`bash-142`, epoch`20260920T113321.556124313-3137898`, loopback8088,60s,29 original targets,concurrency16. Caddy PID3133670, job`bash-139`, loopback18089. DSH PID2909089 remained unchanged.

Executable SHA-256 `500ce33bc259255e123944a45bf7707257738413038a2851021b552a97b8d48d`, copied from the accepted process into `jev-runtime-recovery-2026-09-20/jev-sentinel-verified`. The launcher pins that hash; the mutable `target/debug` binary differs and is not used. Inventory SHA-256 remains `d4396d4efa3ba8793f80272ee3ccace59137a3ee2fbdff6aba279c263a9162c2`.

Neither process has systemd/autostart. Runtime is not durable across host restart. Categories/history/key overrides remain in memory. Separate clean-runtime receipt records the final check; earlier screenshots contain the temporary acceptance category and must not be described as clean-state screenshots.

## Gateway and differential security review

In-session review only, no independent reviewer. Scope: new generator/effective private config, request flow, body limits, logs and local/network negative tests. No production source changes in this publication step.

Ubuntu package `2.6.2-6ubuntu0.24.04.3` extracted without system installation; package SHA-256 matched apt metadata: `3a71aa97c7d85ea7a285717601306bf45d6f701073875ab689e893fabf6a819c`. Exact config SHA-256 `2f7003b29404ce57dfa1b83e9ba027d9cf0a691402f6ab27180fe265d07457ea`; generator `8ad8186008f5a0d6703c39fa0299768df9cf5308f010e87e5ab04e5629a89672`;16-case check `c6a57cd09203511b046bacf183ec4623fe9a5ac8351f70452dbdc4eb38abd3da`.

Serve v1.102.3 removes incoming Tailscale identity headers and derives user identity through WhoIs. Caddy permits exact owner login/host/forwarded host/scheme from loopback. Admin API/autosave disabled. POST only on three mutation endpoints, exact Origin and JSON content type, no cross-site/same-site POST, explicit Content-Length0..16384. GET navigation remains usable. Upstream CORS stripped; no-store/nosniff/frame-denial/limited CSP configured. Header deadline3s, body5s, header8KiB, body16KiB, response-header20s/write25s. Sensitive identity/auth/cookie log-field filters configured and validated; every filter was not separately exercised against a final error log event.

`caddy validate` and final `check-gateway.py` passed (16 cases). Local wrong/missing identity, invalid host/scheme/origin/method/preflight were403; oversized body413 before proxy. Local simulated headers test Caddy matching only, not remote identity. Actual owner access is independently shown above. No second real Tailnet account was available to prove its rejection end to end.

Known limitation: incomplete POST closes in5s without category change but may return empty200 due to Caddy's canceled-context behavior. Initial non-200 assertion failed; final evidence records the narrower deadline/non-mutation guarantee, not a correct HTTP error. UI JSON parsing rejects an empty response. This limitation was not concealed or fixed by test-only response rewriting.

Local host processes are trusted: they can reach8088 or forge gateway headers. Native Rust auth/CORS/whole-body deadline findings remain unresolved inside the backend. Do not expose8088. No proof of security against a compromised host, no public-internet testing, no load/DoS campaign.

Sources: [Serve implementation](https://github.com/tailscale/tailscale/blob/v1.102.3/ipn/ipnlocal/serve.go), [Caddy2.6.2 proxy](https://github.com/caddyserver/caddy/blob/v2.6.2/modules/caddyhttp/reverseproxy/reverseproxy.go), [server limits](https://caddyserver.com/docs/json/apps/http/servers/).

Final clean receipt at14:47:28 MSK: cycles12→15 advanced every target's sequence and measured_at, preserved all29 category assignments, five defaults, no custom key, minimum15 history points;26 healthy/3 unknown. Port8098 is no longer listening. Running executable hash matches the accepted frozen binary; DSH PID remains2909089. Receipt: `clean-runtime.json`.

## Observation and coverage limits

26 healthy/3 Unknown in accepted cycles. Debian/Windows/Omarchy exec commands emit plain text, not required JSON; no probe payload or status was fabricated. Jev assignment/diagnosis can be imperfect despite typed responses. No Safari/Firefox, physical mobile/AT/native-speaker certification, human usability acceptance, Telegram or remediation test.

Delivery gate: original inventory/actions/real cycles on exact consumer URL PASS; bounded gateway checks PASS with stated timeout-status caveat; prose grounding PASS; broader production security and independent review NOT VERIFIED. [Current handoff](../docs/handoff-2026-09-20.md).
