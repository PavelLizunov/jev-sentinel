# Jev Sentinel documentation

Start with the [project README](../README.md) for installation and the dashboard/API contract.

| Document | Use it for |
|---|---|
| [Operator guide](operator-guide.md) | Search, filters, map, display modes, resource metrics, freshness and troubleshooting |
| [Deployment and limits](deployment.md) | Safe access, configuration, CLI side effects, build/asset updates and production blockers |
| [Systemd setup](../contrib/systemd/README.md) | Exact system and user service paths and activation commands |
| [Next-chat handoff](handoff-2026-09-20.md) | Current implementation, real-data preview, evidence and decisions left to the owner |

## Verification and research

These are dated reports, not promises that later snapshots have the same results:

- [P0–P1 implementation verification](../.dsh/verification-p0-p1.md)
- [Localization verification](../.dsh/i18n-verification.md)
- [Approved UX fixes verification](../.dsh/ux-fixes-verification-2026-09-20.md)
- [Production security findings](../.dsh/security-review-report.md)
- [UX differential security review](../.dsh/ux-fixes-security-2026-09-20.md)
- [Local recovery of the original Sentinel runtime](../.dsh/runtime-recovery-verification-2026-09-20.md)
- [Protected publication: consumer acceptance and limits](../.dsh/publication-verification-2026-09-20.md)
- [Historical snapshot checks — not task acceptance](../.dsh/real-data-docs-verification-2026-09-20.md)
- [Engineering references](../.dsh/research/production-references-2026-09-20/REPORT.md)
- [Published operator-experience research](../.dsh/research/user-experience-2026-09-20/REPORT.md)

Published operator reports and direct owner feedback are evidence inputs, not a completed usability study. Browser tests establish particular behaviors, not general convenience or accessibility certification.
