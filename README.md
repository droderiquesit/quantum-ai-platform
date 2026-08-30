# Quantum Investment Platform

A continuously-learning autonomous investment intelligence platform, built as a
machine investment organisation rather than a trading bot.

**Live trading is disabled by default and cannot be enabled from inside the
platform.** The shipped autonomy level is paper trading, the deployment ceiling
starts there too, and raising it requires two authenticated operators plus a
deliberate infrastructure change. See [Safety](#safety) below.

## The loop

```
SENSE → UNDERSTAND → DISCOVER → REASON → SIMULATE → DECIDE → ACT → LEARN
  ↑                                                                  │
  └──────────────────────────────────────────────────────────────────┘
```

One pass is a *cycle*. Every event in a cycle carries the same correlation id,
so any decision can be reconstructed from the event log by a single key.

`Platform::run_cycle` never panics and never stops early: a stage that fails
records its problem and the cycle continues, because LEARN is what would
eventually notice that a stage keeps failing.

## Two brains

| | Fast Brain | Deep Brain |
|---|---|---|
| Horizon | microseconds to minutes | minutes to months |
| Work | market data, microstructure, real-time risk, execution | research, causal reasoning, simulation, optimisation, learning |
| Language model | **never** — enforced at start-up | permitted |
| Reaches a venue | yes, through the execution agent | no |

`qip-fastbrain` refuses to start if any agent it hosts holds
`call_language_model` or has a budget permitting one. A fast path that blocks
on a model call is not a fast path, and discovering that under load is
expensive.

## Safety

Seven controls, each enforced by code rather than by policy:

1. **Paper trading by default.** `AutonomyLevel::DEFAULT` is `PaperTrading` and
   the deployment ceiling starts there. A platform never configured for live
   trading cannot reach a live level even with two authenticated operators.
2. **Two operators to go live.** Raising to any live level requires an
   authenticated operator with a second approver, a stated reason, and a
   credential authenticated within the last fifteen minutes.
3. **No self-escalation.** No agent, of any role, may hold
   `change_autonomy_level`. There is no API endpoint for it either.
4. **Separation of duties.** A research agent cannot submit an order; whoever
   proposes cannot approve; the adversarial reviewer reports to a different
   owner from the analysts it reviews and cannot publish theses of its own.
   `AgentManifest::validate` refuses the combinations outright.
5. **Every number has a provenance.** An agent's numeric facts are *observed*
   or *computed*, and there is no third variant. A language model has nowhere
   to put a number.
6. **An asymmetric kill switch.** Tripping needs no authority, because a false
   stop costs far less than a missed one. Clearing needs an operator with a
   credential authenticated within the last fifteen minutes, and every lift is
   recorded against the trip it lifted.
7. **The book and the venue must agree.** A fill the order state machine
   refuses is recorded as a reconciliation break rather than discarded, and
   reported on `/health` and `/orders`. Positions that quietly diverge from the
   venue's are the failure nothing else downstream would catch.

The venue credential is unreadable in any environment whose autonomy ceiling is
paper trading — not because the application declines, but because the IAM
binding does not exist.

## Dependencies

`serde` and `serde_json`. Nothing else.

Every numeric routine, the HTTP server, the hash functions, the RNG, the
statevector simulator and the HMM are written in-tree. That is a deliberate
trade: more code to maintain, in exchange for a supply chain small enough to
audit, a build that works offline, and no transitive dependency that can change
what the platform computes.

`scripts/check-dependencies.sh` enforces it in CI, so adding a third dependency
is a decision that appears in a diff.

## Layout

```
backend/         the Rust workspace: Cargo.toml, toolchain pins, and
  crates/
    libs/        shared, dependency-light, no I/O side effects
    services/    one per stage of the loop
    agents/      the eighteen-agent investment organisation
    quant/       signals and the strategy SDK
    runtime/     the composition root
    apps/        the four deployables
    tests/       workspace-level acceptance, infrastructure and documentation
frontend/        the browser layer and npm workspace root
  portal/        the authenticated console and installed PWA
  landing/       the public site
  mobile/        the mobile channel (the phone app is the portal PWA)
  packages/      shared browser packages
data/            data domain: local run state, datasets when they exist
infrastructure/
  terraform/     GKE, networking, secrets, alerting
  kubernetes/    manifests, default-deny network policy
vendor/
  templates/     licensed template packages (reference assets)
docs/
  architecture/  how it fits together
  adr/           why it is the way it is
  operations/    runbooks
  developer/     how to work on it
```

## Building

```sh
cd backend
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

The toolchain is pinned in `rust-toolchain.toml`. The build needs no network
access beyond fetching the two dependencies.

## Running

```sh
# The operator command line.
cargo run -p qip-cli -- status
cargo run -p qip-cli -- cycle 3
cargo run -p qip-cli -- governance

# The API and operator interface. Refuses to start without a credential.
QIP_TOKEN_OPERATOR=$(head -c 32 /dev/urandom | base64) cargo run -p qip-api
```

## Documentation

* [Architecture](docs/architecture/README.md) — how the pieces fit together
* [Decisions](docs/adr/README.md) — why they fit together that way
* [Operations](docs/operations/README.md) — runbooks, including what to do when
  the kill switch trips
* [Development](docs/developer/README.md) — conventions and how to add a stage

`backend/crates/tests/qip-acceptance/tests/documentation.rs` checks that what these
claim matches what the code does. Documentation that has drifted from the code
is worse than none, because someone will believe it.
