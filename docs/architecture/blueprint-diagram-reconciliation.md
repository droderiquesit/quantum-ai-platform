# The two authoritative references, compared

ADR 0022 makes **both** the Algorik Master Blueprint v10.1-4 (the DOCX) and its
companion architecture diagram (`index.html`) the architecture of record. Two
authoritative documents describing one system will disagree somewhere, and the
standing rule here is that a disagreement is recorded with both locations
rather than silently resolved in favour of whichever was read last.

Neither reference is edited, and neither is copied into this repository. This
file records only where they differ and where they agree.

**Method.** Each topic below was checked in both references by reading the
relevant section and the relevant diagram region. Agreement is recorded as
well as conflict, because "we checked and they match" is the finding that
stops the next reader re-deriving it.

## Where they agree

| Topic | DOCX | Diagram | Verdict |
|---|---|---|---|
| The seven planes | §1.2, §5 — Ingestion, Cognition, Valuation, Intelligence, Optimisation, Execution, Ledger/wallet/treasury | "The seven planes" band, same seven in the same order | **Agree**, including the argument that Cognition is its own plane |
| The four gates | §51.1 — end of Phases 2, 3, 6, 8 | "The gates, and what a no means" — same four, same consequences | **Agree**, including the Phase 2 wording "Stop. Do not build execution infrastructure. The most important gate in the document" |
| Ten-level risk envelope | §25.3; §39 layer 6 "the envelope at ten levels" | "Risk engine — The envelope at ten levels including causal-driver concentration" | **Agree** |
| Bounded domains | §5, domains numbered to 34 | "34 Bounded domains" | **Agree** |
| Twelve-item payload | §41.5, twelve rows | "12 Items shipped to each region" | **Agree** |
| Runtime substrates | §41.4, §41.6, §45.1 — Cloud Run and Jobs, one GCE C3 class, no Kubernetes | "0 Kubernetes clusters, in any phase"; "~70 Services on Cloud Run and Jobs, scale to zero"; "3 Dedicated VMs — execution nodes only"; "GCE C3 · systemd" | **Agree**, emphatically on both sides |
| Model authority | §39 — "No model has decision authority anywhere" | "0 Models with decision authority" | **Agree** |
| Cognition service set | §41.6 — nine named services | "cognition 9 services", same nine names | **Agree** |
| Cost, early phases | §52 — "TOTAL (approximate) $1,015" for Phases 0–4 | "the pre-revenue phases stay near a thousand dollars a month" | **Agree** |
| Trust zone count | §46.1 — thirteen rows | "Thirteen zones, default deny between them" | **Agree** on the count |
| Global vs regional | §4.2 table | Per-item "per region" annotations on cycle selection, path assignment, capital placement | **Agree** |

## Conflicts

Each needs an owner decision or a reference correction. None is resolved here.

### K1 — the diagram disagrees with itself about total software latency

| Where | Claim |
|---|---|
| Diagram, statistics band | "**665 µs** Software time, tick to order" |
| Diagram, "Latency budget, drawn to scale" | "**~670 µs** — everything the platform controls ends here" and "**670 µs** Internal, wire to dispatch" |

Five microseconds, and the two figures are a few hundred pixels apart in the
same document. The DOCX derives no single tick-to-order total: §26.3 gives
"STRATEGY ENGINE TOTAL < 70 µs" and says it sits on "a hot path already
spending roughly six hundred", which is consistent with either but decides
neither.

**Consequence for this repository: none today, and that is the point.** No
end-to-end latency has been measured here, and `docs/plan/current-state.md`
makes no latency claim. Whichever figure is correct must not be quoted as a
property of this platform until something measures it.

### K2 — risk gate and intent netting budgets differ between the references

| Stage | DOCX §53 walkthrough | Diagram latency block |
|---|---|---|
| Risk gate, whole cycle | step 19: "**+14 µs**" | "**15 µs** Risk gate, whole cycle" |
| Intent netting | step 18: "**+4 µs**" | "**7 µs** Intent netting" |

The netting figure differs by 75%. The DOCX number is drawn from a worked
example with "no opposing intent... a net intent of one contributor" — the
cheapest possible case — so the two may be measuring different things rather
than contradicting. **That distinction is not stated in either reference**,
which is itself the defect: a budget and a best case that look identical will
be quoted interchangeably.

### K3 — what the application and identity trust zone may reach

| Reference | Permitted reach |
|---|---|
| DOCX §46.1 | "Ledger — read. Capital engine, Treasury and Lifecycle — raise intents only. **Never a node, a venue, a QPU or a key**" |
| Diagram, "Trust zones" | "The only path from an interface to platform data. Reaches **Intelligence**, Ledger — read" |

Two differences, and this is the highest-consequence conflict of the three
because it is a security boundary:

1. The diagram grants reach to **Intelligence**, which the DOCX does not list.
2. The diagram omits the DOCX's "raise intents only" qualifier on Capital
   engine, Treasury and Lifecycle — the qualifier that makes the application
   tier unable to *act*, only to ask.

A reader implementing from the diagram would build a wider application zone
than a reader implementing from the DOCX. **Resolve in favour of the narrower
reading until an owner says otherwise** — that is the fail-closed default this
repository applies everywhere else — but the conflict is real and is not
resolved by that convention.

**Bearing on this repository:** the narrower reading is what is built today.
`qip-api` composes reads and holds no independent financial state, and ADR 0018
puts the console on the VPC as viewer. Nothing here reaches Intelligence to
write.

### K4 — the ingestion zone's name and scope

| Reference | Zone |
|---|---|
| DOCX §46.1 | "Ingestion — Source adapters, extraction, entity resolution" |
| Diagram | "Ingestion and discovery — ... and the discovery crawler ... Headless rendering and the dark-web crawler run in the isolated discovery enclave" |

The DOCX puts discovery isolation in §46.2's controls rather than in the zone
table. Not a contradiction — the diagram folds a control into the zone it
constrains — but the zone rosters differ verbatim, so a count or a
name-matching check across the two will disagree. Recorded as a presentation
difference rather than a substantive one.

## What this changes in the repository

Nothing, today. Every conflict above is either about a figure this platform has
not measured (K1, K2) or about a boundary where the repository already
implements the narrower of the two readings (K3, K4).

The value of the pass is that four disagreements between two documents that are
both authoritative are now written down with both locations, so the next reader
finds a record instead of picking one.

## What was not compared

Stated so the coverage of this pass is not overread:

- The eight execution paths (§31) against the diagram's path illustrations.
- The asset class registry (§17.7) and settlement calendars (§17.8) against the
  diagram's asset coverage band.
- Per-plane service rosters beyond Cognition, which was spot-checked and
  matched. The other eleven rosters in §41.6 were not compared name by name.
- The deep-web source tier (§7.6) against the diagram's source categories.
