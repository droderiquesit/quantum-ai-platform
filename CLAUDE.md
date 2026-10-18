# PEOS Quantum AI — working agreement

Multi-regional AI and quantum research platform for investment decisions.
**Strictly paper trading. It never submits a live order.**

@.claude/rules/00-enterprise-governance.md
@.claude/rules/01-security-and-safety.md
@.claude/rules/02-change-management.md
@.claude/rules/10-product-direction.md

## What this is

A Rust workspace that senses markets, reasons about them with a panel of
agents, sizes positions, executes against a simulator, and scores itself
afterwards. Eight stages run in one cycle: SENSE, UNDERSTAND, DISCOVER,
REASON, SIMULATE, DECIDE, ACT, LEARN — count them, because this sentence said
"Seven" until 2026-09-06 while listing all eight, and it is the first thing
every agent reads. `sed -n '/pub enum Stage/,/^}/p'
backend/crates/runtime/qip-kernel/src/cycle.rs` is the check.

Intended users are the research and risk desk running it — not external
customers. The business outcome is a decision loop whose every step is
reproducible and attributable after the fact.

**Non-goals.** Live order submission. Retail distribution. A trading venue. A
general-purpose ML platform. Beating a benchmark is not a goal of the software;
being able to say precisely why it did what it did is.

## Stack, as found in the tree

- **Rust 2024**, resolver 3, one workspace, 66 crates under `backend/crates/*/*` on 2026-10-17
  (`ls -d backend/crates/*/* | wc -l`; recount, do not quote). `unsafe_code = "forbid"`;
  `todo!`/`unimplemented!`/`panic_in_result_fn` denied; no `unwrap()` outside
  tests.
- **Dependencies by allow-list** (ADR 0109, accepted 2026-10-11). Crates come
  from `scripts/dependency-allowlist.txt`, pinned, each line naming the ADR that
  admitted it; the lockfile no longer holds only `serde`, `serde_json` and their
  closure: `./scripts/check-dependencies.sh` printed "42 third-party package(s),
  all permitted" on 2026-10-17 (recount; do not quote the figure). `tokio` is the one async runtime, kept off the
  decision core and the cell's pass unless a committed measurement justifies it;
  explicit timeouts on every call that leaves the process.
- **Next.js + TypeScript** in `frontend/portal/` and `frontend/landing/`
  (their own toolchains — see `frontend/CLAUDE.md`).
- **Google Cloud**: workloads run on Cloud Run (every warm binary) and
  Compute Engine (one execution node per region, ADR 0024, never yet
  applied). The only GKE cluster is ADR 0036's GitOps control plane, which
  runs Argo CD, Kargo and Config Connector and no trading binary. Secrets mounted
  as files from Secret Manager, KMS, Binary Authorization, Workload Identity
  Federation. Terraform 1.9.8, `hashicorp/google ~> 6.12`.
- **GitHub Actions**: `ci.yml` (gate), `deploy.yml` (build/sign/attest/deploy),
  `infra.yml` (plan/up/down).
- **Quantum path** (as of 2026-10-17): QAOA runs against `SimulatedProvider`
  (`qip-kernel/src/platform.rs`, `router.with_quantum(Arc::new(SimulatedProvider::new(..)))`).
  `HostedProvider` (`qip-quantum/src/provider.rs`, `connected` at `:441`) is the
  HTTP seam for a hosted vendor but has no production caller, and no Qiskit
  code is in the tree (`grep -rli qiskit backend --include=*.rs` finds only
  mentions in comments and tests). Nothing in the tree submits an IBM Quantum job. The local steepest-descent fallback and the classical
  baseline computed every time (ADR 0006) are real.

## Layout

Four top-level domains, plus the repo-wide concerns (ADR 0016):

| Path | What lives there |
|---|---|
| `backend/` | The entire Rust workspace — `Cargo.toml`, `Cargo.lock`, toolchain pins |
| `backend/crates/libs/` | Shared, dependency-light, no I/O side effects, except that `qip-transport` owns sockets under ADR 0100 (see `.claude/rules/architecture/00-boundaries.md`) |
| `backend/crates/services/` | Domain engines — ingestion, risk, portfolio, execution |
| `backend/crates/runtime/` | `qip-kernel`: the cycle and the composition of everything |
| `backend/crates/apps/` | Deployable binaries: api, fastbrain, deepbrain, edge-node, web, cli, fabricd, ledgerd |
| `backend/crates/edge/` | Regional cell: routing, order book, sequencing, envelopes |
| `backend/crates/tests/` | `qip-acceptance` — cross-cutting suites |
| `frontend/portal/` | The authenticated Next.js console and installed PWA |
| `frontend/landing/` | The public landing application — the front door |
| `frontend/mobile/` | The mobile channel — see its README: the phone app is the portal PWA |
| `frontend/packages/` | Shared browser packages (brand, design tokens, …) |
| `data/` | Data domain: local run state, datasets and catalogues when they exist |
| `infrastructure/` | Terraform root and modules, per-environment tfvars, the egress bootstrap, Dockerfile, the GitOps manifests — no workload on Kubernetes; the one GKE cluster is ADR 0036's Argo CD/Kargo control plane |
| `vendor/templates/` | Licensed template packages — reference assets (ADR 0015) |
| `docs/adr/` | Architecture decisions. Read before proposing a change one covers |
| `docs/ops/` | Threat model, policies, observability notes |

## Commands

```
make check      # fmt-check, lint, test, deps, secrets — the gate
make all        # check + build + audit + sbom + infra
cd backend && cargo test --workspace --no-fail-fast   # --no-fail-fast matters
cd backend && cargo clippy --workspace --all-targets  # must be zero warnings
./scripts/check-dependencies.sh           # must say "all permitted"
./scripts/check-secrets.sh                # must say "nothing found"
```

## Principles

1. **Say why.** Doc comments and commit messages name the failure the code
   prevents. A comment restating the code is worse than none.
2. **Refuse rather than guess.** Validate inputs; do not clamp them. A value
   silently corrected is a caller bug that survives.
3. **Fail closed.** Every safety default is the restrictive one, and a
   configuration that would relax it stops the process rather than lowering it.
4. **Make it structural.** A guarantee the type system holds beats one a
   runtime check holds, which beats one a comment asserts.
5. **Compute a classical baseline every time** a quantum path runs (ADR 0006).
6. **Bill what ran, not what was planned.** Two independent claims about the
   same fact will disagree, and the louder one will be wrong.

## Definition of Done

See `.claude/rules/02-change-management.md`. In short: every applicable gate
ran, you read its output, and you quoted the evidence. A gate that did not run
is reported as not run.

## Where the rest lives

- Domain rules — `.claude/rules/domains/`
- Architecture rules — `.claude/rules/architecture/`
- Agents — `.claude/agents/` · Skills — `.claude/skills/`
- The six-level map — `docs/claude/SIX_LEVEL_SYSTEM.md`
- The delivery loop — `docs/claude/AUTONOMOUS_DELIVERY_WORKFLOW.md`
