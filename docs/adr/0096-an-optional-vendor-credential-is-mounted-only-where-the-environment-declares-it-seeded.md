# ADR 0096: An optional vendor credential is mounted only where the environment declares it seeded

- **Status**: Accepted on the authority the owner delegated on 2026-09-19
  (ADR 0081's status line records the delegation). **Nothing is applied by
  this record**: it changes what a plan would render and what a manifest
  says, and `dev`'s next apply is where it takes effect.
- **Date**: 2026-09-21
- **Supersedes**: nothing.
- **Related**: ADR 0040 (the owner authorises the `dev` apply, and what that
  authorisation cannot reach — the line this record is downstream of), ADR
  0034 and ADR 0041 (the venue sources and that a registration is one named
  operator's attributed act), ADR 0036 (the manifest is the artefact and the
  catalogue entry is its source of truth), ADR 0003 (paper trading, which
  this does not touch at any of its three layers), ADR 0024 (secrets reach a
  process as files, never as environment values).

## Context

`qip-dev-api` could not start. Cloud Run refused every revision:

```
Secret projects/.../qip-alpaca-api-key-id-dev/versions/latest was not found
Secret projects/.../qip-alpaca-api-secret-key-dev/versions/latest was not found
```

Both containers exist. `infrastructure/terraform/main.tf` creates them in
every environment, deliberately, so that `GET /registrations` can print a
command a person can actually run — `gcloud secrets versions add
qip-alpaca-api-secret-key --data-file=-` — rather than a runbook step that
fails at the moment somebody finally follows it. Neither holds a *version*,
because nobody has registered with Alpaca, and the owner is the only party
who may write one. `catalogue.tf` says so in as many words: **"No value is
created here, ever"**.

So the immediate fix an incident reflex reaches for — put something in the
slot — is the one thing this platform forbids. A fabricated vendor
credential is a value nobody can attribute, in a container whose whole point
is that a named person put it there under terms they read.

The real defect is upstream of the empty slot. `infrastructure/gitops/envs/<env>/api.yaml`
mounted both containers **unconditionally**, and a Cloud Run revision cannot
start when a mounted secret has no enabled version. That turns "no market-data
credential" into "no API at all": the console, `/api/v1`, the health path,
every route with nothing whatever to do with market data, down — waiting on a
commercial relationship nobody has entered into.

Read the comment that shipped beside those two mounts and the error is
visible in it:

> And a Cloud Run revision cannot start on a secret with no enabled version,
> so the slot must be filled before the manifest beside this reconciles; that
> is the same order the runbook already gives (fill the slot, then deploy)

Every clause of that is true. The conclusion is wrong. "Fill the slot, then
deploy" is the right order for somebody *registering with Alpaca*; stated as
a property of the manifest it makes registering with Alpaca a precondition
of serving anything at all. The ordering constraint was real and its scope
was not.

## Decision

**A secret whose version is supplied out of band is mounted only where the
environment declares that the version exists.**

1. A new root variable, `seeded_vendor_credentials` — a list of mount keys,
   `[]` by default and `[]` in all four committed environments. Two
   validations: a name outside the known set is refused at plan time, and so
   is a name given twice.
2. `catalogue.tf`'s `api` entry renders each Alpaca mount inside a
   `contains(var.seeded_vendor_credentials, "<key>") ? { … } : {}` arm — the
   same shape `optional_config_files` already uses for a null root variable,
   so the entry stays one map and the rendering is what differs.
3. The manifests drop both mounts, in all four environments, in all three
   places a mount appears: the `_FILE` environment variable, the
   `volumeMount`, and the `volume`.
4. `main.tf` still creates both containers in every environment,
   unconditionally. The container and the mount are now separate decisions,
   which is what lets the runbook's command keep naming something real while
   no revision depends on what is behind it.

**Nothing is granted by this and nothing is loosened.** The Cloud Run
module's `secretAccessor` grant is keyed on `var.secret_mounts`, so a mount
that is not rendered is a grant that is not made: the API's identity loses
read on two secrets it could not use. That is a narrowing.

## What still refuses, and where

The property that makes this safe is that the refusal was never at the mount.
Traced rather than asserted:

- `qip_core::secret::resolve_from` returns `Ok(None)` when neither
  `QIP_ALPACA_API_SECRET_KEY` nor `QIP_ALPACA_API_SECRET_KEY_FILE` is set,
  and refuses a file that exists and is empty rather than returning it —
  so an unmounted slot and a mounted-but-blank one are both "no credential",
  never "a credential that happens to be nothing".
- `qip_api::feed::credential_is_readable` turns that `None` into a refusal
  naming the slot **and** its `_FILE` variant, at
  `Api::readmit_connector` — the moment an operator approves a registration
  for the source this process senses. It is the seam an operator actually
  meets, and it says the registration stands and is in the log.
- Before either, `qip_data_finder`'s licensing gate and the registration
  registry refuse `alpaca-daily-bars` outright: its requirement is `account`
  and the shipped registry records nobody.
- No composition root reads the Alpaca slots at start-up. `qip-api`'s `main`
  reads `QIP_API_TAPE_PATH`, `QIP_CONNECTOR_SOURCE` and
  `QIP_CONNECTOR_BASE_URL`; no environment sets any of them, so every
  deployment's feed is `ApiFeed::None`, which the banner says out loud.

Removing the mount therefore removed a credential and **enabled nothing**.
The one behaviour that changed is that the process starts.

## What it costs

- **A tfvars line can now be wrong in the optimistic direction.** Naming a
  slot that was never seeded reproduces exactly the dead revision above. The
  list is a claim about the world — "`gcloud secrets versions add` has been
  run" — and Terraform cannot check it, because checking it would mean
  reading Secret Manager at plan time from a configuration that plans without
  credentials. The mitigation is the order the runbook already gave and now
  gives explicitly: seed, then name the slot, then deploy.
- **The set of admissible names is written in two places** — the validation
  in `variables.tf` and the `contains` arms in `catalogue.tf`. A third
  optional credential means editing both. This is deliberate: free text here
  would make a typo render no mount, which is byte-for-byte what leaving the
  name out does, and an operator would have no way to tell a typo from a
  decision.
- **A seeded environment's first apply moves an IAM grant as well as a
  mount.** The grant follows `secret_mounts`, so naming a slot both mounts
  the secret and grants the API read on it. That is the correct coupling and
  it is worth knowing it is a coupling.
- **Two more moving parts in the highest-consequence file in the tree.** The
  answer is that both halves are planned rather than read as text; see below.

## Alternatives rejected

- **Seed the slots with a placeholder.** Refused, and not on grounds of
  taste. A value in that container is indistinguishable, to everything
  downstream, from a credential a named person obtained under terms they
  read; `catalogue.tf` forbids it; ADR 0040 puts the value outside what any
  agent's authorisation can reach. It would also *work* — the revision would
  start — which is what makes it the dangerous option rather than merely the
  wrong one.
- **Delete the mounts and say nothing.** Simplest, and it leaves no declared
  path back: the day somebody registers, the mount returns as an
  unreviewable diff that nobody can connect to the act that justified it.
  The flag makes the connection a reviewed line in the environment's own
  file.
- **Make the mount conditional on `venue_registrations_file` instead.**
  Tempting, because the two are about the same event. Wrong, because they
  are about different *facts*: the registrations file says a record exists,
  the slot holds a credential, and a deployment can legitimately be in either
  state without the other. Gating one on the other would mount an empty
  container the moment a record was committed.
- **Tolerate a missing version at the Cloud Run level.** There is no such
  setting. `versionRef: latest` on an empty container is a start-up failure
  and nothing in the manifest can soften it.
- **A per-venue rather than per-slot flag.** Alpaca's credential is two
  containers and they can be seeded a minute apart. A per-venue flag would
  mount both on the strength of one, which is the original failure with a
  smaller blast radius rather than a fix.

## Evidence

`infrastructure/terraform/tests/seeded-vendor-credentials.tftest.hcl` plans
both halves, with mocked providers and no credential, because
`.claude/rules/domains/infrastructure.md` requires a validation change to
prove that the gate fires on a bad value **and admits a good one** — a gate
that refuses everything reads identically to one that works:

- an environment seeding nothing plans the envelope key (the premise) and
  neither Alpaca half;
- an environment seeding both plans both, at the exact path the `_FILE`
  variable names, and the fast brain still carries the envelope key alone;
- an unrecognised name and a repeated name each stop the plan.

`gitops.rs::every_run_service_holds_the_invariants_its_catalogue_entry_and_the_cloud_run_module_held`
now reads the gate out of the catalogue and holds each manifest to it in both
directions — a gated mount is required where the environment names the slot
and **refused** where it does not — and counts the conditional decisions it
made, so a walk that stopped recognising the arm fails instead of quietly
checking nothing.

`qip-api/tests/feed.rs::a_credential_slot_with_nothing_behind_it_refuses_the_source_and_names_both_places_it_looked`
holds the refusal that has to survive the mount being withdrawn, and asserts
its premise first: the same call admits when the file is there.

## What would make this wrong

- A path by which an absent credential becomes something other than a
  refusal. If any caller ever reads `Ok(None)` from `qip_core::secret` as
  "this check does not apply", this record's argument collapses and the
  mount is no longer the safe thing to withdraw.
- A workload whose *start-up* legitimately requires a vendor credential. None
  exists — the capital-envelope key is the only secret any root refuses to
  start without, and it is seeded by Terraform rather than by a vendor — but
  such a workload would need the mount unconditional and the failure moved to
  a place an operator can read.
- Enough optional credentials that the two-place name list stops being
  reviewable. At that point the set belongs in one `locals` map that both the
  validation and the catalogue read, which is a mechanical change this record
  does not pre-authorise.

## Paper trading

Untouched, and checked rather than assumed. Layer one:
`autonomy_ceiling`'s validation in `variables.tf` is unmodified and
`terraform/tests/paper-boundary.tftest.hcl` still plans all six rungs. Layer
two: `AutonomyLevel::deployable` is not in this diff. Layer three:
`qip-edge`'s `Cell` and `qip-cost-router`'s `Determinism` are not in this
diff. Every manifest still carries `QIP_AUTONOMY_CEILING: paper_trading` as a
literal and `live_capable: 'false'` as a label. The credential this record is
about is a **market-data** credential; no venue credential, no order path and
no broker is named anywhere in it.
