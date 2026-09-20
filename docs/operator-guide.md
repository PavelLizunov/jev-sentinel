# Operator guide

The dashboard displays **probe observations**, not an inferred inventory of every host and VM. Several checks may refer to one machine. A probe's identity is its configured type plus unique name; changing its name changes identity.

## Find a probe

1. Type part of its name, ID, type or category into Search. Matching is case-insensitive. A localized category label also matches.
2. Read the active restrictions next to Search. Category and **Issues only** can hide an otherwise matching probe.
3. If matching probes are hidden, use **Show matches across all probes**. It keeps the search text and clears category/Issues only.
4. **Clear filters** also clears the query and resets grouping. It does not change the language or display mode.

`012` finds `demo-node-012` only in the synthetic 100-probe fixture. It is not the name of an actual homelab system. In the real-data snapshot, search for `mac-mini`, `harness-test` or `pve-ninitux` instead. No result is a valid outcome when a substring is absent.

View, language and filter preferences are stored in that browser/origin. An old search may survive reload. Storage refusal does not prevent use. API keys are not stored in browser local storage.

## Map and list

- The map starts open on desktop and mobile. **Hide map / Show map** is an explicit toggle; selecting a map cell opens details.
- The map and summary represent **all observations**, regardless of list filters. The map has its own bounded, keyboard-focusable scroll region.
- **Cards** show resource values and small history charts. **Compact** retains the values with fewer charts. **Table** supports scan/sort; the mobile table keeps name, state, Swap and latency, not every desktop column.
- Cards and Compact order resources vertically: CPU, RAM, used Swap. RAM is not repeated.
- Results use ordinary page scrolling. Short or empty result sets retain at least one viewport of height below the controls to stop filtering from moving the search field. The blank space is intentional.

## What the states mean

| State | Meaning |
|---|---|
| Healthy | This probe's latest observation satisfied its check; not proof that every service on the machine is healthy |
| Degraded | The probe reported a degraded observation |
| Failed | A failure was observed, such as an unexpected HTTP response status |
| Unknown | The observation cannot establish service condition; includes timeouts/unreachable transport |
| Stale | The last observation exceeded its freshness limit; not a new failure measurement |

HTTP dashboard connectivity, observation age and Jev advisor availability are separate. A successful refresh does not renew old measurements. A failed refresh retains last-known data with a disconnected warning.

In the daemon, the stale limit is `max(30 seconds, 2 × interval_seconds)`. Ages use monotonic collector time. History is in memory: 128 samples per probe, latest 20 published. A daemon restart clears history. Polling the page does not create samples.

## CPU, RAM and Swap

- `—` means unavailable; `0` is a measured zero. Missing advice is likewise not zero risk.
- Used Swap is displayed in **MiB** in every mode. No percentage is invented when the total is absent. Allocated swap capacity and free swap-pool space are not used Swap.
- Memory interpretation appears in Details. macOS `free + inactive` is an availability estimate, not a pressure measurement. A probe's reported RAM and a VM's configured allocation are different quantities.
- Proxmox aggregate arrays stay explicit resource rows in Details. They are not attached to a host by a similar-looking name. Host RAM and guest allocations must not be added as though they were independent capacity.
- History uses real elapsed spacing and explicit gaps. It is not a prediction, root-cause diagnosis or an availability SLA.

## Languages and keyboard

The header supports English, Russian, German, French, Spanish, Brazilian Portuguese, Simplified Chinese and Japanese. Locale changes do not change numbers, IDs or observation times in the data model. Non-English text is machine-translated and not native-speaker certified.

Tab reaches interactive controls; Enter/Space activate buttons; Escape closes native dialogs. Automated Chromium checks cover these paths and mobile widths. Screen readers and physical mobile keyboards were not acceptance-tested.

## Categories and the Jev key

In the actual Sentinel daemon, **Add category** stores a category definition and makes it available in the filter. **Categorize with Jev** sends the current probes and category criteria to the provider and updates their assignments. **Jev key** verifies and saves a manual-categorization override; **Use server key** clears the override. Daemon health evaluation continues to use its configured key.

Definitions and assignments survive collection cycles, but categories and the manual key override are in memory and reset on daemon restart. These controls require a trusted authenticated connection; verifying a key and classifying probes make real provider requests.

## Preview versus deployed daemon

The separate review preview is read-only. Key/category/inference controls are disabled and mutations rejected; it does not run collectors, Jev, Telegram or remediation. Its banner identifies the source.

A **real-data snapshot** is a fixed export with an explicit date. It can contain genuine CPU/RAM history but is not live monitoring. Time continues to pass, so the snapshot becomes stale; refresh never fabricates new observations. Its `beszel_snapshot` identities are separate from configured Sentinel probe identities. Registry status `up` is retained only as source metadata, not promoted to a service-health verdict. Missing Swap remains unavailable.

A **synthetic preview** is explicitly labeled synthetic and offers four explained data sets in **Demo data**: a 100-probe overview, stopped updates, unavailable advisor, and no configured probes. These controls do not manage real systems.

The current handoff records which preview is running: [handoff](handoff-2026-09-20.md). For ongoing operations, use the actual monitoring system rather than a fixed review snapshot.

## Troubleshooting

| Symptom | First check |
|---|---|
| Name exists but search looks empty | Active category and Issues only; use hidden-match recovery or Clear filters |
| Map has more probes than the list | Expected: map is unfiltered |
| Swap is `—` | Whether the source actually supplied used Swap; absence is not zero |
| Everything is stale | Source observation times and whether this is a fixed snapshot; HTTP 200 is not freshness |
| Jev is unavailable while metrics are visible | Advisor evaluation is independent of collection; read the error, do not infer probe failure |
| Updated files do not appear in daemon UI | Assets are embedded; rebuild and explicitly deploy/restart the correct binary, not DSH |

See [deployment and limitations](deployment.md) before exposing the daemon beyond loopback.
