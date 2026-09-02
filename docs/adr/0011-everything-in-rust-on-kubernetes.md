# 0011 — Everything in Rust on Kubernetes; IBM Quantum is the only integration

**Status:** superseded by ADR 0024 in its "on Kubernetes" half — the runtime is Cloud Run and one execution node per region, provisioned in code and not yet applied; the "everything in Rust" half stands.
**Supersedes in part:** ADR 0009, which tiered the dependency policy to admit
Google client libraries at the I/O edge. No such client is now permitted.

## Decision

The platform is built entirely in Rust and runs on Kubernetes. **IBM Quantum is
the only external service it integrates with.**

Specifically, and these replace what the architecture diagram names:

| The diagram says | What is actually built |
|---|---|
| Google Pub/Sub — global streaming backbone | `qip-transport`: an in-tree HTTP/1.1 client and mesh transport |
| Spanner — global database | `qip-storage`: an in-tree embedded storage engine with a write-ahead log |
| BigQuery — analytics | The hash-chained journal, queried in-process. **See "What this costs" — this is the weakest substitution** |
| Vertex AI — model training | `qip-training`: in-tree ridge, boosted stumps, and distillation |
| Dataflow — pipelines | `qip-streaming`: in-tree processing and sequencing |
| GKE — compute | Unchanged. Kubernetes is the one platform dependency, and any conformant cluster serves |

Google Cloud remains a *host*: GKE, VPC, Artifact Registry, KMS, Secret
Manager, a storage bucket and IAM. Those are infrastructure primitives every
provider has. Nothing in the running platform calls a Google API.

## Why

Three reasons, in the order they actually weigh.

**Portability.** A platform whose runtime dependencies are Kubernetes and
nothing else can move — to another provider, to colocation next to a venue, to
a customer's own cluster. For a system whose central argument is
source-adjacency, being pinned to one provider's regional footprint is a
strategic cost, and the Chicago and New York cells already demonstrate it:
Google has no region in either metropolitan area, and the runbook records that
those cells sit 400km and 330km from the venues they are meant to be next to.

**Auditability.** The supply chain is eleven third-party packages, all `serde`.
Every number the platform computes can be traced to code in this repository. A
managed client pulls a transitive tree too large to read, into the process that
moves money.

**Latency.** A network hop to a managed service is exactly what an edge cell
exists to avoid. The hot path already refuses I/O; keeping the transport and
the store in-process is consistent with that rather than an exception to it.

## What it costs

The costs a managed service absorbs do not disappear. They convert into
engineering effort and operational risk, and this platform now owns both.

**Reliability engineering is ours.** Retries, backpressure, at-least-once
delivery, dead-lettering, ordering guarantees — every one of these is now code
in `qip-transport` that has to be right, rather than a service level somebody
else maintains. The transport therefore states its guarantees exactly and
refuses to claim exactly-once.

**Durability engineering is ours.** Write-ahead logging, fsync discipline,
crash recovery and torn-write handling are now code in `qip-storage`. The
failure mode is the dangerous kind: not a crash you notice, but rare silent
corruption that surfaces as a number nobody can reconcile months later. That is
why the engine's tests truncate the log at many offsets rather than one, and
why it states its durability guarantee as a sentence the code must match.

**Analytics is the weakest substitution, and it is named here rather than
discovered later.** A hash-chained journal is an excellent audit record and a
poor analytical database. "Every decision across nine cells for two years,
grouped by regime" has no engine to run against, and building a query planner
and a columnar store is not on this roadmap. If that question becomes load
bearing, the honest answer is to export the journal to something built for it —
and that is the one place this ADR expects to be revisited.

**No multi-writer, no replication.** The embedded engine is single-node. High
availability comes from Kubernetes restarting a pod against a persistent
volume, not from a replicated consensus group. A region losing its volume loses
that region's local state, and the recovery path is a restore, which somebody
must write and rehearse.

## What would make this wrong

Three conditions, any one of which should reopen it.

1. **Analytical queries become load bearing.** See above. This is the most
   likely one.
2. **The reliability code becomes the source of incidents.** If the transport's
   retry and backpressure logic is where outages originate, then the trade was
   bad and a managed bus is the cheaper answer regardless of dependency count.
3. **Multi-region consistency becomes a requirement rather than a
   convenience.** Nine cells with independent local state and an eventually
   consistent roll-up is the current design. If the platform ever needs a
   single global transaction across regions, an embedded single-node engine
   cannot provide it, and Spanner exists precisely because that problem is
   genuinely hard.

## What does not change

The autonomy ceiling stays at paper trading and nothing here raises it. The
decision core keeps its two dependencies. IBM Quantum remains asynchronous and
offline — never in the critical execution path — and remains a port that
reports itself unavailable until a deployment supplies the token, the service
instance and a transport.
