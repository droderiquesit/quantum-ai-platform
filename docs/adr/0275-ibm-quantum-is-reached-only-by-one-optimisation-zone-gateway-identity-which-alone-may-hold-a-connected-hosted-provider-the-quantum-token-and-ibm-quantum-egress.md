# ADR 0275: IBM Quantum is reached only by one optimisation-zone gateway identity, which alone may hold a connected hosted provider, the quantum token and ibm-quantum egress; the deep brain submits through QuantumGateway and holds none of the three

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation (`docs/blueprint/owner-decisions-2026-10-31.md`),
  on decision 14's ruling for R12-QUANT-9040
  (`docs/blueprint/lanes/wave1031-handoff-2-2026-10-31.json`, lane "L4").
  The number is from lane L4's block (decision 15). The delegation does not
  lift four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them, and it contracts no vendor.
- **Date**: 2026-10-31
- **Keeps**:
  - ADR 0006: a classical baseline every time.
  - ADR 0150: quantum families stay unbuilt without a benefit record.
  - ADR 0008: no model is consulted over the network on the reflex path.
  - The trust-zone egress table in `infrastructure/terraform/modules/trust-zones`.

## Context

R12-QUANT-9040 asks that IBM Quantum be reached only through a controlled,
asynchronous quantum gateway. Its acceptance is an inspection: only the
gateway identity has egress to IBM hosts, the gateway queues jobs, and a
second caller fails the check.

Most of this is already in the tree:
- The queue is `QuantumGateway`. `submit` puts each job on a worker thread
  and `poll` reads it without blocking. It refuses inline delivery, and the
  router submits through it.
- The transport to a hosted vendor is `HostedProvider::connected`
  (`qip-quantum/src/provider.rs`). It has no production caller, and
  `runtime_reachability` lists it as expected to be unwired.
- The trust-zone table sanctions `ibm-quantum` egress for the
  `optimisation` zone alone. No workload in `catalogue.tf` sits in that
  zone. `qip-deepbrain`, which links the optimisation engine and so builds
  a `QuantumGateway`, sits in `cognition`.
- The `qip-quantum-token` secret is created and mounted on no workload.

What was undecided is which identity may hold the three things that reach
IBM, and what refuses a second one.

## Decision

1. **One identity reaches IBM Quantum: a gateway workload of its own in the
   `optimisation` zone.** It alone may:
   - construct a connected `HostedProvider`;
   - mount `qip-quantum-token`;
   - hold `ibm-quantum` egress.

   No such workload is declared, because none may be until an owner
   contracts a vendor (R12-QUANT-9007). Declaring one without a contract
   would provision billed infrastructure for nothing to call.
2. **The deep brain stays in `cognition` with no route to IBM.** It submits
   quantum work through `QuantumGateway`, whose provider today is the
   in-tree simulator. When the gateway workload exists, the deep brain's
   provider becomes a client of that workload, never a connected
   `HostedProvider` of its own.
3. **The second caller is refused by a guard built now.**
   `qip-acceptance`'s `arch_033_quantum_path` test
   `only_the_optimisation_zone_gateway_identity_may_build_a_connected_hosted_provider_or_mount_the_quantum_token`
   reads two things:
   - every crate's production source, for a delimited
     `HostedProvider::connected(`. It allows a hit only in `qip-quantum`'s
     own source and in the crate of a binary that `catalogue.tf` places in
     `optimisation`, a set that is empty today.
   - `catalogue.tf`, for a `qip-quantum-token` mount on a workload outside
     `optimisation`.

   Either hit fails the build, naming the crate or the workload and its
   zone. The egress half is already pinned by the infrastructure suite's
   `the_trust_zones_deny_by_default_and_only_optimisation_may_reach_ibm`.

## Consequences

- R12-QUANT-9040 stays PARTIAL. The identity exists only as a rule: no
  workload, no mount, no egress. That is the state the ruling chose, and
  it holds until a vendor is contracted.
- A change that wires IBM into any existing binary fails the acceptance
  suite instead of passing review on a reading of the diff.

## What it costs

- A gateway workload of its own, when it comes: one more service, one more
  identity and one more network hop between a solver and IBM. That is the
  price of the token and the egress existing in exactly one place.
- A guard that reads Terraform from a Rust test. It parses `catalogue.tf`'s
  workload blocks by their layout, so a reformatting of that file can make
  the guard refuse to run. It fails closed in that case: its premises
  assert it found the known workloads before it judges anything.

## What would make this wrong

- A vendor whose job submission cannot sit behind a separate gateway
  identity, for example one that binds credentials to the calling
  process's own workload identity. Then the identity question needs its
  own record.
- A second quantum vendor. The rule is written for IBM's egress name, and a
  second one needs its own table entry and its own decision about which
  identity holds it.
