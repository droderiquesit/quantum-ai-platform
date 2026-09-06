# 0049 — §48's toolchain is scored as three rows, and adopting any of the three tools is a separate decision this record does not take

**Status:** *proposed*, 2026-09-06. DEC-D11 asks whether the traceability matrix
gains rows for blueprint §48 and rule 77 — OpenTofu, Cloud Build, Cloud Deploy.
It has been *blocked-external* on "the matrix owner's call" since ALIGN-A1, and
the reason it stayed there is that the row and the adoption were being asked as
one question. **They are two.** Scoring what the tree does is answerable today,
from the tree, and this record answers it. Whether to adopt any of the three
tools is a decision with real costs, and this record refuses to take it and
states the costs instead.

**Concerns:** DEC-D11 in `../plan/PROJECT-PLAN.md` and
`../plan/completion-plan.md:411`; the missing rows in
`../architecture/algorik-blueprint-traceability.md`, whose §48 appears only in
the heading "The seven layers (§40.5, §41, §45, §46, §47, §48)" at `:443`.

**Relates to:** [ADR 0017](0017-gitops-delivery.md) (the first delivery path),
[ADR 0024](0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
(its retirement), [ADR 0036](0036-argo-cd-and-kargo-return-on-a-control-plane-cluster.md)
(its return, which changed the facts under this row),
[ADR 0022](0022-the-algorik-blueprint-is-the-architecture-of-record.md) (why a
departure from §48 needs scoring at all).

**Changes no file.** The rows below are drafted for the matrix owner;
`docs/architecture/` is outside the paths this record's author may edit, and a
matrix row written by someone who cannot then re-score the surrounding table is
a half-edit.

---

## What the inconsistency actually is

Not between an ADR and the tree, and not between two ADRs. **Between two
registers carrying the same identifier for the same subject in two different
states**, plus a stale factual assumption inside one of them.

`../architecture/deployed-vs-blueprint.md:648` carries a D11 with an answer:

> | D11 | Whether §48's OpenTofu / Cloud Build / Cloud Deploy row is CONTRADICTS
> or transitional | Transitional: GitHub Actions and Terraform stay;
> `gcloud run deploy` and a MIG update from the workflow stand in for Cloud
> Deploy's rollout. Cloud Deploy's gradual rollout with automatic rollback
> (§48) is not reproduced |

That table is headed "Decisions this document does not make … Each is the
owner's. The assumption column is what the parallel engineer is proceeding
under" (`:634-638`). So it is an **assumption**, not an answer — but it reads
like one, and the project plan simultaneously carries DEC-D11 as
*blocked-external*. A platform that has an assumption which reads as a decision
and an open row saying no decision exists will eventually cite whichever suits.

**And the assumption's factual half is now false.** ADR 0036 moved the rollout
off the workflow:

> What this job did until ADR 0036 — `gcloud run services update`, the serving
> proof, the images.tfvars write and its commit — belongs to the reconciler
> now.
> — `.github/workflows/deploy.yml:397-399`

> Read-only. This job no longer commits anything: Kargo's promotion commit is
> the record.
> — `.github/workflows/deploy.yml:431-432`

So nothing in the workflow "stands in for Cloud Deploy's rollout" any more. The
stand-in is Kargo and Argo CD, which is a different substitution with different
properties — and which does not run: ADR 0036 is partly applied, the control
plane's cluster exists tainted, no controller runs, and no `RunService` has
been reconciled.

---

## Decision, part one: the three rows, and what each says

Scored against the tree as of 2026-09-06. Each row scores the **tool** and the
**property** separately, because a reader who learns only that a tool is absent
has learned nothing about whether the guarantee exists — and it is the
guarantee the blueprint is buying with the tool.

### Row 1 — §48 "Infrastructure", rule 77

The blueprint asks for two things in one line:

> Infrastructure is OpenTofu and emits GCP and IBM resources only.
> — `../architecture/algorik-blueprint-v10.1-source.md:5284-5285` (rule 77)

> OpenTofu. Plan reviewed before apply. State in Cloud Storage with locking.
> Emits GCP and IBM resources only
> — `../architecture/algorik-blueprint-v10.1-source.md:4306-4307` (§48)

**Tool: ABSENT, deliberately.** Terraform 1.9.8 — the version `infra.yml`
pins — with `hashicorp/google ~> 6.12`, both named in
`.claude/rules/domains/infrastructure.md` as the approved set.

**Properties: MET.** Plan reviewed before apply — `infra.yml`'s split
plan/up/down, the guard hook that refuses an unreviewed apply, and the rule
that an agent shows the plan and a person applies. State in Cloud Storage with
locking — `backend "gcs"` with `prefix = "qip/state"`
(`infrastructure/terraform/main.tf:47-48`). GCP resources only — the provider
set is `google` and `google-beta` and nothing else; no IBM resource is emitted
because the IBM integration is an API call, not a provisioned resource.

**Score: TOOL-SUBSTITUTED, properties met.** Not CONTRADICTS: nothing in the
platform's behaviour differs. Not ALIGNED: a reader looking for OpenTofu will
not find it.

### Row 2 — §48 "Build and test", and Cloud Build

**Tool: PARTIAL — and this is the row the absence of a matrix row has been
hiding.** §48's builder *does* run here, for two workloads:
`scripts/deploy-frontends.sh` builds the portal and the landing with
`gcloud builds submit`, signs each digest with the pipeline's attestor and key
version, and reads back the routed revision (`:1-20`, `:32-40`). The four Rust
images are built by GitHub Actions in `deploy.yml`.

`../architecture/deployed-vs-blueprint.md:246` scores this line "ABSENT" with
the qualifier in its evidence column; a matrix row that copied the score
without the qualifier would say the platform does not use Cloud Build, which is
false, and false in the direction that hides a live dependency on a Google
service in the two workloads that face the internet.

**Score: PARTIAL — Cloud Build for the two browser surfaces, GitHub Actions for
the four Rust images.**

### Row 3 — §48 "Deploy — services", and Cloud Deploy

> Cloud Deploy with gradual rollout and automatic rollback on error rate
> — `../architecture/algorik-blueprint-v10.1-source.md:4308-4309`

**Tool: ABSENT.** **Property: NOT REPRODUCED, and not by Kargo either.** ADR
0036's path promotes by commit, with a post-sync hook that asks whether the
revision is Ready and whether the routed revision runs the attested digest, and
fails the sync when the answer is no (`deploy.yml:399-408`). That is a *proof
gate before traffic*, which is a different guarantee from *gradual rollout with
automatic rollback on error rate*: nothing rolls back on an error rate, and
nothing produces one to roll back on — a search for `error_rate` and `error
rate` across `infrastructure/` returns no file, and `workload_metrics_exist` is
`false` in every environment, so no series exists for a policy to read. Naming Kargo as the substitute without
that sentence would score a guarantee the platform does not have.

**And none of it runs.** ADR 0036 is partly applied; no controller runs; the
last mechanism that moved a Cloud Run service was released from the workflow
and not yet taken up by the reconciler. The honest row says the platform's
delivery is, today, *manual dispatch plus a build that records what it pushed*.

**Score: ABSENT, property NOT REPRODUCED, substitute declared and not
running.**

---

## Decision, part two: nothing is adopted, and that is left open on purpose

This record does **not** propose adopting OpenTofu, Cloud Build for the Rust
images, or Cloud Deploy. It does not refuse them either, because refusing them
permanently is as much a decision as taking them and it is the owner's. What it
does is put the costs where the next reader will find them.

**OpenTofu.** The migration is mechanical and the gain today is zero: every
property §48 asks for is already met by Terraform. The cost is not the
migration, it is everything pinned to the word `terraform` — `infra.yml`'s
version pin, the guard hook's command matching, `make infra`, the
`infrastructure` acceptance suite, the provider lock, and every runbook. A
change that alters no behaviour and touches every gate is the kind that gets
half-done. The case for it is licensing posture and governance of the tool
itself, which is a judgement about a vendor rather than about this platform.

**Cloud Build for the Rust images.** It would move the build off the runner
that holds the Workload Identity Federation identity and performs the
attestation. Binary Authorization is the platform's one working asymmetric
signature path (ADR 0043), and moving the builder moves who signs. That is a
security-boundary change wearing a tooling change's clothes, and it needs the
attestation story rewritten before the builder is swapped, not after.

**Cloud Deploy.** It would be a *fourth* delivery path in a repository that has
churned through three — ADR 0017's Argo CD and Kargo, ADR 0024's retirement of
them for a push from `deploy.yml`, ADR 0036's return of them on a control-plane
cluster. Each churn cost a working path and left a record explaining why the
last one was wrong. The threshold for a fourth should be the guarantee §48
actually names — automatic rollback on an error rate — and that guarantee needs
a measured error rate, which needs ingestion, which is ALIGN-A6, which is
blocked upstream. **Cloud Deploy is not blocked on a decision; it is blocked on
observability the platform does not have.** That is the useful finding in this
row and it is why the adoption question was unanswerable in the shape DEC-D11
asked it.

---

## The alternatives, and why they were not taken

**(a) Leave DEC-D11 blocked-external.** The status quo since ALIGN-A1.
Rejected for the reason ADR 0043 gave for DEC-D2: a row that cannot close
because it bundles an answerable question with an unanswerable one is
mis-stated, not blocked. Splitting it costs one record and closes half.

**(b) Score the three lines CONTRADICTS.** Rejected: a contradiction is where
the platform does something the blueprint forbids. Terraform meets every
property rule 77 states except the tool's name; Cloud Build is *used*; and the
Cloud Deploy line is an absence with a declared substitute. Scoring absence as
contradiction inflates the count that the matrix exists to keep honest.

**(c) Score them ALIGNED because the properties are met.** Rejected for row 3,
where the property is genuinely not met, and rejected in general because it
would let a tool substitution disappear from the record. ADR 0022 made the
blueprint the architecture of record; a departure from it is a thing to name,
not to normalise.

**(d) Copy `deployed-vs-blueprint.md:648`'s assumption into the matrix.**
Rejected: it is stale in its factual half (`deploy.yml` no longer deploys) and
it is labelled in its own document as an assumption the engineer proceeds
under, not a decision. Promoting an assumption to a scored row by copying it is
how an unowned sentence becomes a citation.

**(e) One row rather than three.** Rejected: the three lines have three
different answers — properties met, tool partially used, guarantee absent — and
a single row would have to pick one of them and be wrong about the other two.
That is precisely how "ABSENT" came to stand for a Cloud Build the platform
actually runs.

---

## Where this sits in the layering

No code, no crate, no dependency and no direction. The subject is which tools
build and deliver the artefacts, which is an infrastructure and pipeline
concern by construction: nothing in `backend/crates/**` learns what built it,
and no lib, service, runtime or app gains an edge from any row above. The one
place a change here would reach the platform's own guarantees is the
attestation identity — named under Cloud Build's cost — and this record moves
it nowhere.

---

## What it costs

- **Three rows to keep current.** A scored row is a maintenance obligation; the
  Cloud Build row in particular will be wrong the day the frontends move to the
  same pipeline as the Rust images, and nothing will fail when it does.
- **Half a decision is still half a decision.** Splitting DEC-D11 closes the
  scoring and leaves an adoption question that a future reader may find in a
  weaker state than they expected, because the record they found says "answered
  in part".
- **Naming Cloud Build's real use invites a question nobody has asked**: two
  internet-facing workloads are built by a Google service, from a script run on
  a developer's own gcloud login, and their attestation comes from the same
  attestor the pipeline uses. That is defensible and it is not the same posture
  as the four Rust images, and this record makes the difference visible.
- **The Cloud Deploy finding is a dependency on blocked work.** Recording that
  the guarantee needs an error rate ties a delivery decision to ALIGN-A6, which
  is blocked upstream on an image nobody can currently ship. Two open rows now
  reference each other.

## What would make this wrong

- **A row being written that scores a tool without its property, or a property
  without its tool.** That is the failure this record exists to prevent, and it
  has already happened once in the "ABSENT" score for a Cloud Build that runs.
- **The frontends moving off Cloud Build**, which would make row 2's PARTIAL
  false in the safe direction and still false.
- **A controller actually reconciling on the ADR 0036 path.** Row 3's "declared
  and not running" is the accurate score today; the day Kargo promotes and Argo
  CD syncs, the row becomes "substitute running, guarantee still not
  reproduced", which is a different sentence.
- **Someone citing this record as a refusal of OpenTofu.** It is not one. It
  says the gain is zero today and the cost is every gate, and it leaves the
  call where DEC-D11 put it.
- **An error-rate signal appearing.** If ingestion is ever proven and an error
  rate becomes measurable, the Cloud Deploy question stops being blocked and
  becomes a straight comparison against ADR 0036's path — at which point this
  record's part two should be re-asked rather than quoted.
