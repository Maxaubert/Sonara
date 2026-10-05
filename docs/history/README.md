# Sonara history

Finished work, kept for the record. Nothing here describes the current product: the Python
daemon (`sonari`, then `sonara` up to 0.10) is retired, and the Rust runtime replaced it in 0.11
(#202). For current docs see [../README.md](../README.md).

This folder is listed in the root `.ignore`, so rg and Grep skip it. Search it on purpose with
`rg --no-ignore-dot <pattern> docs/history` (or open the files directly).

| Folder | What |
|---|---|
| `plans/` | Implementation plans, 2026-06 to 2026-07 (Python phases 1 to 3, Kokoro, ducking, settings page, session manager) |
| `specs/` | The design specs those plans implemented |
| `audits/` | Code audits from 2026-07 and the Phase 0 audit (2026-10-01) |
| `spikes/` | Early technical spikes (key injection) |
| `verification/` | Clean-room install checklist (phase 3.1) |
| `mockups/` | Settings page mockups |
| Top-level files | Windows acceptance runs (M2, M3), friend test round 1, phase 1 to 3 logs and smoke checklists, the eyes-free prompts spec, the M2 Windows API reference |
