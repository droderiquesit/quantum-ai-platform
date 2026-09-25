# The blueprint of record

The architecture this repository is scored against, since [ADR 0099](../adr/0099-blueprint-v11-6-and-gcp-v2-1-are-the-architecture-of-record-in-direction-and-every-standing-decision-they-contradict-keeps-its-force-until-its-own-record.md).
It is held here verbatim, so that every requirement ID can be traced back to a
page someone else can open.

## Sources

| File | What it is | Pages | SHA-256 |
|---|---|---|---|
| `source/algorik-master-blueprint-v11.6.pdf` | Master Architecture & Application Blueprint v11.6 (its running header says v11.5) | 45 | `1d9bf2a7d3c3e70cde2ac6834b703b17851171c5f7b8b371b110b7f8996cb42c` |
| `source/algorik-gcp-platform-blueprint-v2.1.pdf` | GCP Full Platform Architecture Blueprint v2.1 (its running header says v2.0) | 24 | `048731dae45b6169b58fd3c15989190f0774f9c005f816026ad2f089d286c600` |
| `source/algorik-full-platform-architecture-v2.1.html` | Interactive architecture diagram, eight views | — | `3cb8ad56b8388784e213bffd88a57cb94a1695a630d885cc10416f4ae5565d0a` |
| `source/algorik-master-blueprint-v11.6.txt` | `pdftotext -layout` of the master PDF, with `=== page N ===` markers | 45 | `f7429547668ea7fae0856766788d639e0fd2e337dfa28ff859e32d3c4f6888e1` |
| `source/algorik-gcp-platform-blueprint-v2.1.txt` | The same for the GCP PDF | 24 | `9d3eba25a6d53ed61f7a97369d7fda2886db7c922bcdc3ff5cf1933b558ab2f8` |

The `.txt` files exist so that requirements can be cited by page and found
with `grep`. They are derived from the PDFs mechanically. If a `.txt` and its
PDF ever disagree, the PDF is the source. Check them with
`sha256sum docs/blueprint/source/*`.

## Precedence

1. v11.6 governs **what** the system does.
2. GCP v2.1 governs **where and how** it runs on Google Cloud. Where it is more
   specific about a v11.6 requirement, the specific statement is the
   requirement.
3. The diagram is an index to the other two and adds edges (who talks to whom,
   synchronously or not) that the PDFs describe only in prose.
4. The rules files and the paper-trading layers outrank all three. A
   blueprint requirement that needs live capital or external action is scored
   `BLOCKED`, not built (ADR 0099, "The paper-trading boundary").

## What lives here

- [`requirements.md`](requirements.md) — every requirement, one ID each,
  rendered from `requirements/*.json`, which is the machine-readable source.
- [`traceability-matrix.md`](traceability-matrix.md) — one row per requirement:
  target, current implementation, status, gap, dependency, priority,
  verification and work item.

## Requirement IDs

`DOMAIN-NNN`. The domain is one of the 31 codes defined in
`requirements.md`. IDs are assigned in blueprint source order within a domain
and are **never reused**. A requirement found to be wrong is marked
`withdrawn` with a reason rather than deleted, because a work item or a commit
may already cite it.
