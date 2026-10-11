# The blueprint of record

Master Blueprint **v12.0** and GCP Full Platform Blueprint **v3.0**, held here
verbatim so that every requirement can be traced to a page someone else can
open. Adoption is the subject of ADR 0101 and ADR 0107.

v12.0 deletes no section of v11.6: it carries v11.6's text word for word and
adds §9.6–9.14, §11.1–11.5, §13.1–13.3, §14.1–14.3, §22.1, §24.9 and §31.1
(`v12-delta-master.md`). The v11.6 and GCP v2.1 sources were removed on
2026-10-11 for that reason, together with the requirement catalogue and
assessment decomposed from them. They are in git history at `f65c2c48`.

## Sources

| File | What it is | Pages | SHA-256 |
|---|---|---|---|
| `source/algorik-master-blueprint-v12.0.pdf` | Master Blueprint v12.0, "Financial Superintelligence" — supplied 2026-10-04 | 51 | `4350e26e8973945ae310bfd95e4f9c7a7a7193e33b5eb9b57a56ea713fe366a2` |
| `source/algorik-gcp-platform-blueprint-v3.0.pdf` | GCP Full Platform Blueprint v3.0 — companion to v12.0 | 30 | `8b877fa83446000b5ab560cd27303ad4980b0e133a81ee9295244daf6ac349c8` |
| `source/algorik-full-platform-architecture-v3.0.html` | Interactive architecture diagram v3.0 | — | `66c0f025b6328984f7ca665037ec4ebd453c35bd62b42f57b1a5082bcfba2942` |
| `source/algorik-master-blueprint-v12.0.txt` | `pdftotext -layout` of the v12.0 PDF, with `=== page N ===` markers | 51 | `a2ff5d8a6cb63039fc543e8b6d740e41cbc9b4b87eb62548f087396d53cc9ee7` |
| `source/algorik-gcp-platform-blueprint-v3.0.txt` | The same for the GCP v3.0 PDF | 30 | `e9e39b673e6be4e3ed605bcd92c2c4e41dcfa9f3b0b58c50752e817aea4f238e` |

The `.txt` files exist so that requirements can be cited by page and found
with `grep`. They are derived from the PDFs mechanically. If a `.txt` and its
PDF ever disagree, the PDF is the source. `qip-acceptance`'s
`every_blueprint_source_is_listed_once_and_has_the_hash_the_readme_records`
holds this table to the bytes on disk, in both directions.

`docs/architecture/algorik-blueprint-v10.1-source.md` is kept deliberately.
v12.0 §2 is written as corrections to v10.1, and the tables some code is
specified against — §22.1 retention classes, §30.2 path assignments — exist
only there; the conformance tests in `qip-data-finder` and
`qip-acceptance/tests/path_router.rs` read them.

## Precedence

1. v12.0 governs **what** the system does.
2. GCP v3.0 governs **where and how** it runs on Google Cloud. Where it is more
   specific about a v12.0 requirement, the specific statement is the
   requirement.
3. The diagram is an index to the other two and adds edges (who talks to whom,
   synchronously or not) that the PDFs describe only in prose.
4. The rules files and the paper-trading layers outrank all three. A
   blueprint requirement that needs live capital or external action is scored
   `BLOCKED`, not built (ADR 0099, "The paper-trading boundary").

## What lives here

- `source/` — the five files above.
- [`v12-delta-master.md`](v12-delta-master.md) and
  [`v12-delta-gcp.md`](v12-delta-gcp.md) — what v12.0 and v3.0 add to the
  editions before them, page by page. The starting point for the v12.0
  decomposition.
- [`reviews/`](reviews/) — the two reviews of ADR 0101.
- [`v12-completion-plan.md`](v12-completion-plan.md) — the waves of work that take the
  R12 register to COMPLETE, read from the register.

## The register

`requirements/R12-*.json` (1,518 requirements decomposed from v12.0 and GCP
v3.0 on 2026-10-11, ten slices M1–M7 and G1–G3) and `assessment/R12-*.json`
(one row each, scored from code, tests and Terraform), rendered by
`qip blueprint render` to [`requirements.md`](requirements.md),
[`traceability-matrix.md`](traceability-matrix.md) and
[`v12-completeness-targets.md`](v12-completeness-targets.md). Edit the JSON and
re-render; never edit a view. The v11.6 catalogue and assessment it replaced
are in history at `f65c2c48`.

## Requirement IDs

`R12-<DOMAIN>-<N>`. The `R12-` prefix keeps every id clear of the retired
`DOMAIN-NNN` ids that commits and ADRs still cite. N comes from a block per
slice (M1 1001–1999 … M7 7001–7999, G1 8001–8999, G2 9001–9999, G3
10001–10999), in source order within the slice, and is **never reused**. A requirement found to be
wrong is marked `withdrawn` with a reason rather than deleted.
