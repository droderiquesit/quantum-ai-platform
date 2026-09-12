# 0053 — Declared-and-gated managed-service scaffolding does not contradict "not used"

**Status:** *proposed*, 2026-09-12.

**Relates to:** [ADR 0009](0009-tiered-dependency-policy.md) and
[ADR 0002](0002-two-dependencies.md) (why an adapter for any of these three
services does not exist in-tree today — no crate is admitted to speak
AlloyDB's wire protocol, Bigtable's gRPC data plane, or mint the bearer token
Vertex AI's REST surface needs), [ADR 0024](0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
(the general pattern of provisioning ahead of a capability this repository
has used before), [ADR 0022](0022-the-algorik-blueprint-is-the-architecture-of-record.md)
(why a departure from the blueprint needs to be named rather than left
implicit).

**Does not touch:** any Terraform file. `enable_vertex_ai`, `enable_bigtable`
and `enable_alloydb` stay `false` in every environment; this record ratifies
the pattern already written into `infrastructure/terraform/variables.tf` and
`infrastructure/terraform/modules/data/variables.tf`, it does not change it.

---

## Context

`docs/DELIVERY-STATUS.md`'s "Where the blueprint and the code disagree"
recorded this as an open question:

> §44.1 says Vertex AI, Bigtable and AlloyDB are not used. All three are
> declared in Terraform and disabled in every environment.

and §44.1's own scored row went further, calling it a contradiction:

> Contradicted on paper: `grep -rn 'google_vertex_ai_metadata_store\|...'
> infrastructure/terraform/modules --include=*.tf` finds a Vertex AI metadata
> store and endpoint, a Bigtable instance literally named `timeseries`, and
> AlloyDB — the three things this section says are not used. They are inert,
> not removed.

Reading the blueprint text and the Terraform module together shows this is
not, in fact, a contradiction, and shows one more thing: the claim as written
overstates what §44.1 says.

**What §44.1 actually says.** The section
(`docs/architecture/algorik-blueprint-v10.1-source.md:4034-4048`) is a
six-row table of things "not used" and why. Exactly one of the six rows names
a cloud product: "Vertex AI or a model registry product", with reason
"Versioning, lineage and promotion are rows and objects." A second row, "A
time-series database", gives BigQuery-plus-memory as covering the need and
declines "a third tier" — which is the shape Bigtable would fill, but the row
never says the word. **Neither "Bigtable" nor "AlloyDB" appears anywhere in
the blueprint source** — confirmed by
`grep -in 'bigtable\|alloydb' docs/architecture/algorik-blueprint-v10.1-source.md`,
which returns nothing. So the disagreement as filed already claims more than
the specification states for two of its three names; that overreach is
corrected below regardless of how the substantive question is decided.

**What "not used" means at every level that can be checked, for all three.**
Each of the three flags gates a `count = var.enable_X ? 1 : 0` resource:

```
grep -n 'count = var.enable_vertex_ai' infrastructure/terraform/modules/ai/main.tf
grep -n 'count = var.enable_bigtable' infrastructure/terraform/modules/data/main.tf
grep -n 'count = var.enable_alloydb' infrastructure/terraform/modules/data/main.tf
```

and

```
grep -rhoE 'enable_(vertex|bigtable|alloydb)[a-z_]* *= *(true|false)' infrastructure/environments/*/terraform.tfvars
```

is `false` for all three, in `dev`, `test`, `stage` and `prod`. `count = 0`
means Terraform creates **zero instances** of `google_vertex_ai_metadata_store`,
`google_vertex_ai_endpoint`, `google_bigtable_instance.timeseries`,
`google_alloydb_cluster.records` and `google_alloydb_instance.primary`
wherever it runs. Nothing is provisioned, nothing is billed, and nothing
exists for a process to reach — "declared" here means present as HCL text in
this repository, not present as a resource anywhere Google can bill for.

And even the counterfactual — someone flipping a flag to `true` — would not
make the platform use the service, by the module's own documentation. Each
variable's description
(`infrastructure/terraform/modules/data/variables.tf:78-101`,
`infrastructure/terraform/variables.tf:621-624`) states the missing half by
name: AlloyDB "has no REST data plane — its REST API is admin-only — so the
only route to a row is the PostgreSQL wire protocol"; Bigtable's "data plane
is gRPC only, with no JSON surface for rows"; Vertex AI's port "has no client,
no credential and no egress path, so enabling this provisions somewhere to
train without making this build able to submit a job." None of the three has
an adapter under `qip_storage` or `qip-training` capable of speaking to it —
`qip_storage::provider::StorageTarget::is_implemented` returns `true` for six
targets and `false` for exactly these three (`AlloyDb`, `Bigtable`, and
`Spanner`, the last scored separately because it does have a REST surface
and is a judgement call rather than a protocol impossibility). Flipping the
flag would buy "a healthy, empty, billable instance" — the module's own
phrase — not a used service.

**So both documents are describing the same fact from two directions, and
neither is wrong.** The blueprint says these three are not used; nothing in
the tree uses them, at any layer, whether the flag is on or off. The
Terraform module says the same thing in its own words — the flag "means 'an
adapter exists and I have wired it', not 'I would like this service'" — and
adds the reason: writing the resource block now, disabled, lets a future
change turn on a real capability by flipping one line and writing the
adapter, rather than by writing new infrastructure at the same time as new
Rust. **What was actually missing was not a correction to either document,
but a decision on record for why this pattern is acceptable at all**, since
until this record the justification for three declared-and-disabled
resources for capabilities the architecture says it does not use lived only
in Terraform comments, which is exactly the shape `.claude/rules/architecture/00-boundaries.md`
asks to be turned into an ADR: "If you find yourself explaining an
architectural choice in a PR comment, it needed an ADR."

## Decision

**The pattern is ratified: a managed service the blueprint marks "not used"
may still be declared in Terraform, gated behind a flag that defaults false,
provided three things hold — and all three hold today for Vertex AI,
Bigtable and AlloyDB.**

1. **The flag's default is `false` and every environment's tfvars agrees.**
   Re-checked by the grep above at the time of this record.
2. **No adapter exists that could use the service if the flag were flipped.**
   `StorageTarget::is_implemented` (or the equivalent for a non-storage
   service) says so structurally, not just by absence of a call site — a
   flag with a working adapter behind it and a `false` default would be a
   different, weaker claim ("not used yet, but could be with one line"),
   which is not what any of these three are.
3. **The reason the flag exists at all is written down where the flag is
   declared**, naming the protocol or credential obstacle rather than "not
   implemented yet" — which all three already do, in the variable
   descriptions quoted above.

Under those three conditions, "not used" is the accurate description of the
platform's behaviour and its actual cloud footprint, and "declared, disabled,
no adapter" is the accurate description of what is in the tree. They are the
same fact. `docs/DELIVERY-STATUS.md`'s §44.1 row and its "Where the blueprint
and the code disagree" entry are corrected to say so, and the AlloyDB/Bigtable
overreach — attributing a name to §44.1 that the section never uses — is
corrected in the same edit.

## Alternatives considered and rejected

**Remove the three resource blocks from Terraform, so the tree has nothing to
find.** Rejected: `count = 0` already means nothing is created, so removal
would trade a documented, disabled option for no record of the intended shape
at all — the next person who needs tick-history storage would start from
zero instead of from a module with the protocol obstacle already named. This
is also outside this record's paths (`infrastructure/**` is not writable
here) and would be a Terraform change, not a documentation one.

**Amend §44.1's blueprint text to add "(scaffolding exists in Terraform,
disabled)" after each row.** Rejected: it would be true, but it describes an
implementation decision inside a document ADR 0022 named the architecture of
*record* — the destination, not the inventory of what today's tree happens to
contain in a dormant state. That inventory is exactly what
`docs/DELIVERY-STATUS.md` exists to hold, and duplicating it into the
blueprint would give the two documents two copies of one fact that can drift
apart, which `CLAUDE.md`'s sixth principle warns against for exactly this
reason.

**Score §44.1 `CONTRADICTS` and open a task to decide whether to keep the
Terraform scaffolding.** Rejected: scoring a non-contradiction as one inflates
the count `docs/DELIVERY-STATUS.md` exists to keep honest, the same reasoning
ADR 0049 gave for not scoring a tool substitution as `CONTRADICTS` when every
property still held.

**Say nothing and leave the disagreement open.** Rejected for the same reason
as ADR 0052: the document's own charter is to record a verdict, and the
evidence above is sufficient to give one.

## What it costs

**A pattern is now load-bearing rather than merely present.** The three
conditions above are now a standard a fourth declared-but-disabled service
would be checked against, and a future addition that fails one of them —
say, a flag defaulting `true`, or an adapter that partially works — would need
its own record rather than inheriting this one's ratification silently.

**"Not used" now carries a footnote a reader has to know to look for.** A
person reading §44.1 cold still sees "not used" with no indication that three
of its implied services have Terraform scaffolding; the accurate account is
one grep and one ADR away rather than in the same sentence. That is the same
trade-off ADR 0049 made for the OpenTofu row, made here for the same reason:
duplicating the fact into the blueprint costs a second copy that can drift.

## What would make this wrong

- **Any of the three flags defaulting `true` in a committed environment file.**
  That would be using the service, or provisioning one nobody can point at a
  reason for, and this record's condition 1 would be unmet.
- **An adapter appearing for any of the three** (a Rust crate or module
  reaching AlloyDB's wire protocol, Bigtable's gRPC surface, or a Vertex AI
  client with a credential and an egress path) **while the corresponding
  blueprint row still says "not used".** At that point the row would need to
  become a real disagreement — built and reachable versus specified as
  absent — rather than the non-conflict this record describes.
- **A future flag with no obstacle named in its description.** The
  justification here depends on each variable's own text naming *why* no
  adapter exists; a flag added later with a bare `default = false` and no
  reason would not meet condition 3 and should not be read as covered by this
  ratification.
