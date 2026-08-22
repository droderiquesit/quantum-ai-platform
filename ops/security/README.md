# Security

## The threat model

Three attackers, in order of how much this platform's design assumes about
them.

**A compromised dependency.** Mitigated by having two, and by
`scripts/check-dependencies.sh` failing the build if a third appears. See
[ADR 0002](../../docs/adr/0002-two-dependencies.md).

**A compromised agent.** An agent whose code, prompt or model has been
subverted. Mitigated by capability gates that contain an agent which does not
cooperate, by separation of duties that makes the dangerous combinations
unconstructable, and by a numeric provenance rule that gives a subverted model
nowhere to put a fabricated price. See
[ADR 0004](../../docs/adr/0004-capability-gated-agents.md).

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
| Default-deny network | Kubernetes `NetworkPolicy` | applied before workloads |
| No root, no escalation, read-only root | pod `securityContext` | pod security standard `restricted` |

Every row is checked by a test. `crates/tests/qip-acceptance/tests/infrastructure.rs`
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
