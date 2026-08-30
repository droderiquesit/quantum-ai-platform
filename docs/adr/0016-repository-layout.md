# 0016 — One layout: backend, frontend, data, infrastructure

**Status:** accepted

## Decision

The tree is organised as a conventional monolith: one top-level directory per
domain, each answering one question, with repo-wide concerns beside them.

| Directory | Owns |
|---|---|
| `backend/` | The entire Rust workspace — `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.cargo/`, and `crates/` beneath it. Every cargo command runs from here. |
| `frontend/` | Everything a browser runs, and the npm workspace root. Divided by product surface: `portal/` (the authenticated console and installed PWA), `landing/` (the public front door), `mobile/` (the mobile channel — the phone app is the portal PWA; the directory owns mobile-distribution artefacts and documents that decision), `packages/` (shared browser packages), `scripts/` (frontend tooling such as the token-CSS generator). |
| `data/` | The data domain's non-code assets: local run state (`data/local/`, git-ignored), committed datasets and licensed-source catalogues when they exist. Its README states what deliberately does *not* move here — crate test fixtures stay with the crates that read them, and production data lives only in the event log. |
| `infrastructure/` | Terraform, Kubernetes manifests, the Dockerfile, workflows' target. |
| `vendor/templates/` | The licensed template packages (SignalAIX, Cryptrix, the Fortradex documentation bundle) — source assets per ADR 0015, not applications. |
| `docs/` | All documentation, including `docs/ops/` (threat model, policies, observability notes — formerly a top-level `ops/` that held only documentation). |
| `scripts/` | Repo-wide gates only: dependency policy, secret scan, test counting, deploy bootstrap. Tooling owned by one domain lives in that domain. |

## Why the workspace moved, when the first cut of this ADR refused to

The first draft kept `crates/` at the top level, arguing that the rulebook,
CI and 3,000+ tests pointed there and a rename bought alignment on paper at
the cost of churn in everything. The desk overruled it: a top level where
Rust manifests, npm manifests, and five loose config files sit beside the
domain directories makes the *first* question — "where does this kind of
thing go?" — answerable only by tribal knowledge. The churn argument was
real but one-time; the legibility argument applies to every future reader.
All path-bearing surfaces were updated in the same change, which is the only
safe way to do it: `qip-acceptance`'s `repository_root()` (now four levels
up), the Makefile, `ci.yml` working directories and cache keys, the
Dockerfile's `COPY`, and the gate scripts, which now resolve the lockfile
from their own location so they pass or fail identically from any cwd.

Likewise the first draft refused a top-level `data/` as a directory with
nothing in it. The desk wanted the division; the resolution is a directory
whose README defines ownership and explicitly names what stays out, so it is
a boundary statement rather than an empty control.

## Removed, and why each was safe

- `crates/runtime/qip-kernel/Cargo.toml.tmp` — a tracked editor artefact.
- `frontend/mobile/__MACOSX/` — macOS zip metadata that shipped inside the
  Cryptrix package.
- `frontend/logos/` — a byte-identical duplicate of the brand package's
  assets (verified by checksum before deletion); its one unique file,
  `site.webmanifest`, moved into the brand package first.
- root `tests/` — a README describing suites that actually live in
  `backend/crates/tests/qip-acceptance/`; a stale pointer someone would
  believe.

## What it costs

Every existing checkout, branch and muscle-memory path is invalidated at
once: `cargo` commands need `backend/`, npm commands need `frontend/`, and a
rebase across this commit rewrites more file paths than any change before
it. The acceptance suite's root-walking helper and every literal repository
path in it had to move in the same commit, which is exactly the kind of
wide, mechanical diff that hides a real mistake — the mitigation is that
the gates themselves are the moved things, so a missed pointer fails a
build rather than lying quietly. And the Docker image now copies `backend/`
alone, so any future build input placed outside it will be invisible to the
image until someone widens the `COPY`.

## What would make this wrong

If the landing and portal ever merge into one app, `frontend/` collapses to
fewer surfaces. If a real dataset or catalogue never materialises, `data/`
should shrink back to its README rather than accrete unrelated files. If a
second licensed template family arrives, `vendor/` may need per-vendor
licence notes rather than one flat `templates/`.
