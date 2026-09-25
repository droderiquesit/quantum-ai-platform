# Milestones

Each milestone is closed by evidence someone else can re-run, not by an
assertion. Status as of 2026-09-25.

| Milestone | Exit criterion | Status | Evidence |
|---|---|---|---|
| **M0 Baseline** | Repository understood and reproducible | **Done** | [test baseline](../reports/test-baseline.md) (6,276 → 6,284 passing, 0 failing; clippy 0 warnings; portal 188 Playwright passing); [security baseline](../reports/security-baseline.md); deploy blocked by billing, recorded |
| **M1 Blueprint normalised** | Every requirement has an ID | **Done** | 1,566 requirements in `docs/blueprint/requirements/*.json`; `documentation::the_blueprint_views_are_what_their_sources_render_to` holds the views to the sources |
| **M2 Gap analysis complete** | Every requirement mapped to the current implementation | **Done** | [matrix](../blueprint/traceability-matrix.md): 1,566 rows assessed, every batch re-checked by a skeptic (240 downgrades, 135 upgrades); [gap analysis](../reports/gap-analysis.md); [debt register](../reports/tech-debt.md) |
| **M3 Target architecture stabilised** | Interfaces and migration strategy agreed | **Done for the first slice** | ADR 0099 (adoption, conflict register), ADR 0100 (fabric and slice, from a judged three-way design panel); [target state](../architecture/target-state.md); [migration map](../architecture/migration-map.md); 57-packet DAG |
| **M4 Build/CI healthy** | Baseline build and test pipeline functioning | **Done (build and CI); deploy blocked** | CI green on PR #17 (28/28); local gates green; `deploy.yml` fails at image push (billing disabled) — an owner action |
| **M5 First vertical slice working** | One real end-to-end path functions | In progress | ADR 0100 §8's eight real-process tests passing in `SLICE-48` |
| **M6 Core platform functioning** | Major backend domains operate together | Not started | Stage B of the [roadmap](master-roadmap.md) |
| **M7 Blueprint functional completion** | All required capabilities implemented (in paper/shadow form where C1 refuses) | Not started | matrix: every non-BLOCKED row COMPLETE |
| **M8 Reliability/security completion** | Security, resilience, observability, performance gates pass | Not started | game days; SLOs ingested; security review clean |
| **M9 Working reference release** | Runs from clean deployment and completes the end-to-end scenarios | Not started | Needs billing and a C8 cost ceiling |
