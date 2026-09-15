# Security

## The threat model

Three attackers, in order of how much this platform's design assumes about
them.

**A compromised dependency.** Mitigated by having two, and by
`scripts/check-dependencies.sh` failing the build if a third appears. See
[ADR 0002](../../adr/0002-two-dependencies.md).

**A compromised agent.** An agent whose code, prompt or model has been
subverted. Mitigated by capability gates that contain an agent which does not
cooperate, by separation of duties that makes the dangerous combinations
unconstructable, and by a numeric provenance rule that gives a subverted model
nowhere to put a fabricated price. See
[ADR 0004](../../adr/0004-capability-gated-agents.md).

**A caller with a stolen credential.** Mitigated by role-scoped tokens, by
constant-time comparison, by short expiry, and by the fact that the most
dangerous operation — raising the autonomy level — has no API endpoint at all
and requires two people.

## Controls

| Control | Where | Enforced by |
|---|---|---|
| Live trading off by default | `AutonomyLevel::DEFAULT` | a test asserting the constant |
| Deployment ceiling | `PlatformConfig::autonomy_ceiling` | refused at `request_change` |
| Two operators to go live | `OperatorIdentity::with_second_approver` | refused without one |
| Venue credential unreadable in paper environments | Terraform IAM binding | the binding does not exist |
| No self-escalation | `AgentManifest::validate` | refuses `change_autonomy_level` for every role |
| Separation of duties | `AgentManifest::validate` | refuses the combinations |
| Capability containment | `Gated<T>` | private field, checked accessor |
| Numeric provenance | `NumericProvenance` | no third variant exists |
| Constant-time token comparison | `Authenticator::authenticate` | `constant_time_eq`, no early return |
| Request limits | `ServerLimits` | enforced while reading |
| Default-deny network | per-zone firewall rules in `modules/trust-zones` and `modules/execution-node` | priority 65000 deny in both directions |
| No root, no shell, empty filesystem | the image: `FROM scratch`, `USER 10001:10001` | Cloud Run runs what the image says |
| No external address, no container runtime | `modules/execution-node` and its startup script | no `access_config`; the script refuses to start beside a runtime |

Every row is checked by a test. `backend/crates/tests/qip-acceptance/tests/infrastructure.rs`
covers the infrastructure rows; the rest are covered in their own crates.

## Secrets

No secret value is ever in Terraform. Secrets are created empty and their
values written out of band, so a state file that leaks contains the shape of
the deployment and none of its credentials.

Credentials in the application store only a SHA-256 hash. A test serialises a
`Credential` and asserts the token is not in the output.

`scripts/check-secrets.sh` runs on every build. It is deliberately narrow: a
scanner that flags every high-entropy string produces a wall of false
positives, and a wall of false positives is a scanner people learn to skip.

## What an order-entry adapter can and cannot do

Recorded 2026-09-15, after an independent review of the paper-trading boundary
read the REST order-entry adapter as a path by which real orders could leave
the process. It is not one, and the reasoning is written down here because the
alarming version is the one a reader re-derives: the refusal messages in
`qip-edge-node`'s `venue.rs` say, correctly and prominently, that nothing in
the process can tell a venue's sandbox host from its production host. Read on
its own, that sentence sounds like the boundary rests on an operator typing an
endpoint twice. It does not.

`QIP_VENUE_ADAPTER=rest` does construct a `RestGateway`, and that gateway does
hold a credential and a socket. What it cannot do is place an order, and the
guarantee is in the types rather than in a check:

* `run_pass` in `qip-edge-node/src/pass.rs` takes `gateway: &mut
  SimulatedGateway` — a concrete type, not a trait object.
* `Cell::work` has exactly one caller anywhere in `backend/crates/apps/` and
  `backend/crates/runtime/`, and it is inside `run_pass`. Verify with
  `grep -rn '\.work(\|place_cycle(' --include='*.rs' backend/crates/apps
  backend/crates/runtime | grep -v /tests/`, which printed three lines on
  2026-09-15 — read them, do not count them: only `pass.rs`'s is a `Cell`, and
  the two in `qip-kernel/src/adaptive_cadence.rs` are `work()` on a cadence
  plan and are not this method at all. A count here would be the wrong
  measurement, which is why the check is the receiver and not the number.
* The pass runs only under `if let (Some(pass_loop), Some(simulated)) =
  (pass_loop.as_deref_mut(), gateway.simulated_mut())`, and `simulated_mut`
  answers `None` for the live arm.
* `FeedChoice` has exactly one variant, `Simulated`, so the feed is either that
  or absent; and a simulated feed on a non-simulated gateway is refused at
  start-up, before anything is served.

So the two reachable configurations are: a simulated feed, which refuses to
come up beside a REST gateway; or no feed, in which case no pass runs and no
order is ever built. There is no third. `Cell::send`'s own live-class refusal
sits behind all of this as defence in depth.

Two things follow that are worth stating rather than leaving implied.

**A flat `qip_edge_refusals_total{gate="live_venue"}` is not evidence that the
live-venue gate has been exercised.** Every `Placer` this workspace can build
answers `is_simulated() == true`, because `AdapterClass` has only `Simulated`
and `Sandbox` and `is_paper()` is true for both. The gate fires the moment a
class exists that answers false, which is why it is there; it has never fired
outside tests and cannot today. Do not cite a zero counter as a tested control.

**The residual risk is a credential and a socket, not an order.** A node
misconfigured with a production endpoint holds order-entry credentials it
should not and connects to a host it should not. That is worth preventing, and
it is a different and much smaller claim than orders being submitted.

## Reporting

A vulnerability in this platform should be reported privately to the owning
team before any public disclosure. There is no bug bounty.

## What is not covered

* **TLS termination.** The in-tree HTTP server speaks plaintext HTTP/1.1 and is
  intended to sit behind a terminating proxy. Writing a TLS implementation
  would be far more dangerous than depending on one.
* **Live venue connectivity.** The adapter interface is complete; the transport
  is not present in this build. `LiveBroker::requirement` names exactly what is
  missing.
* **Denial of service beyond the request limits.** The server bounds what a
  single connection can allocate and caps concurrency. Sustained
  volumetric attack is a network-layer concern.
