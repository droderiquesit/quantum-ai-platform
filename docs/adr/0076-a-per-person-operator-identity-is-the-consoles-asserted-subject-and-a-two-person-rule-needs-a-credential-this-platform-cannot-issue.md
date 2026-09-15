# ADR 0076: A per-person operator identity is a per-person credential, and a two-person rule needs one this platform cannot issue

- **Status**: Proposed
- **Date**: 2026-09-15
- **Supersedes**: nothing
- **Amends**: ADR 0042, whose decision 6 attaches an assertion to two routes the
  process that would mint it cannot call — see Context, "The carrier nobody had
  named"
- **Related**: ADR 0002 (two dependencies), ADR 0009 (the tiered policy and what
  may never be hand-rolled), ADR 0012 (where a library earns its place),
  ADR 0013 (verifying an identity token earns a dependency), ADR 0018 (the
  console authenticates as `viewer`), ADR 0019 (Identity Platform is the only
  identity store), ADR 0038 (passkeys, proposed), ADR 0041 (an approval is one
  operator's attributed act), ADR 0042 (the console proves who clicked with a
  keyed assertion), ADR 0043 (the three gaps no in-tree code may close),
  ADR 0062 (a venue is reinstated only by two signatures), ADR 0065 (a standing
  secret has no authentication instant), ADR 0075 (the capital-issuance route is
  authorised in shape and refused in fact)

This record is ADR 0075's **Gate A**, taken before the code rather than after
it. ADR 0075 authorised the capital-issuance route in shape and refused it in
fact, on the finding that the route is not the binding constraint: `issue`
refuses below a capital-holding rung, the only path to one is two signatures
from two distinct subjects, and this deployment mints one subject per role. It
named the prerequisite and did not decide it. This record decides it, and the
decision is partly a refusal.

**What is decided here is buildable and small. What is refused here is refused
with a named external component and a named cost, not deferred to a mood.**

## Context

### Gate A is three problems wearing one name

"A per-person operator identity carrying a real authentication instant" reads as
one thing. It is three, they fail independently, and every previous attempt on
this ground has closed one and reported the gap shut:

1. **The subject.** Who the platform records as having acted, and whether two
   humans can ever present two of them.
2. **The instant.** Whether anything in the request is evidence that a person
   was at the keyboard, as opposed to evidence that a secret was held.
3. **The carrier.** By what authority the request arrives at all — which
   credential passes the role gate before either of the above is consulted.

ADR 0065 closed nothing and said so, correctly, refusing rather than fabricating
an instant. ADR 0042 designed the subject and half the instant. Neither touched
the carrier, and the carrier is what makes ADR 0042's design refuse in the
deployment it was written for.

### What the tree does today

Each claim is a command. Run them in the tree you are about to change.

**The subject is a role.**

```
grep -rn '@env' backend/crates/apps/qip-api/src
```

Two lines: the mint, `format!("{}@env", role.as_str())` in `main.rs`, inside the
loop over `QIP_TOKEN_MONITOR`, `QIP_TOKEN_VIEWER`, `QIP_TOKEN_ANALYST` and
`QIP_TOKEN_OPERATOR`; and the doc comment on
`Principal::authentication_instant` that explains what it costs. Both holders of
the operator token present `operator@env`.

**The instant is refused, deliberately.** `qip_api::auth::Presence` has one
variant, `Unattested { credential_minted_at }`, `attested_at()` returns `None`,
and `Principal::authentication_instant(action)` turns that `None` into a
refusal naming the act. Seven routes go through it:

```
grep -n 'authentication_instant(' backend/crates/apps/qip-api/src/routes.rs
grep -n -B3 'required_role: Role::Operator' backend/crates/apps/qip-api/src/routes.rs
```

Every one of the seven is `Role::Operator` — the two ledger decisions, the
registration approval, the three signature-gated approvals
(`/strategies/:strategy/promotion-approvals`,
`/risk/recalibrations/:rule/approvals`, `/venues/:venue/reinstatements`) and
`DELETE /kill-switch`. That fact is the third problem in disguise, and the next
subsection is about it.

**The kernel wants two distinct people and a fresh credential for each.**
`Platform::approve_promotion` refuses a countersignature whose subject equals
the first signer's, by name, before `Approval::countersigned_by` refuses it
again; `ApprovalChain::check` then requires an `OperatorCredential` **per named
approver** and refuses any that is not fresh within
`qip_compliance::approval::MAXIMUM_CREDENTIAL_AGE`, fifteen minutes.

```
grep -n 'a second session is not a second person' backend/crates/runtime/qip-kernel/src/platform.rs
grep -n 'MAXIMUM_CREDENTIAL_AGE\|no authenticated credential was presented' backend/crates/libs/qip-compliance/src/approval.rs
```

**And nothing in production has ever built one of those credentials.**

```
grep -rn --include=*.rs 'OperatorCredential::verified' backend/crates
```

Every construction site is under a `tests/` directory. The one hit under `src/`
is a doc comment in `central/plane.rs` describing what a caller would have to
do — which is the self-matching-citation hazard ADR 0075 warned about in its own
first command, met again here: **read each hit, do not count them.** So
`ApprovalChain`'s per-approver evidence check has only ever run under
`cargo test`, and the fixtures that run it name methods this platform has never
performed — `"webauthn"` and `"hardware-token"`, in
`qip-kernel/tests/central.rs`, `qip-acceptance/tests/region_share.rs`,
`qip-compliance/tests/capital_approval.rs` and two others. The one production
constructor of the kernel-side twin passes `"api-bearer-token"`
(`routes.rs`, seven sites), which is honest. A reader who meets the fixtures
first will believe this desk authenticates with hardware keys. It does not
authenticate at all.

### The carrier nobody had named, and it is the finding of this record

**The console holds the `viewer` token and no other platform credential, on
purpose, and every route above is `Role::Operator`.**

```
grep -rn 'qip-token-viewer' infrastructure/terraform/modules/secrets/main.tf scripts/deploy-frontends.sh
grep -n 'authenticates as .viewer' docs/adr/0018-the-console-reaches-the-platform-over-the-vpc.md
```

ADR 0018 decided it in one sentence — "**The console authenticates as
`viewer`.** … Viewer is the whole entitlement: it reads." The secrets module
grants the console's service account `secretAccessor` on `qip-token-viewer` and
nothing else, under a comment that states the property being bought: "A console
compromise is therefore a disclosure of what this deployment already shows a
signed-in operator, not a control of it." `scripts/deploy-frontends.sh` mounts
`qip-token-viewer-dev` at the path `QIP_API_TOKEN_FILE` names.

The consequence for ADR 0042 is not a detail. ADR 0042's decision 6 attaches its
assertion to `POST /registrations/{source_id}/approve` and
`POST /ledger/users/:user/eligibility`, both `Role::Operator`, and its decision 5
fixes the order of checks as bearer, rate limit, **role**, then assertion. In
the deployment ADR 0042 describes, the role check refuses the console before the
MAC is ever computed. The mechanism is sound and the carrier is missing, and
because the mechanism was designed without the carrier, an implementer following
ADR 0042 to the letter would have built a verified attribution channel onto
routes the asserting process cannot call. This record amends ADR 0042 by
supplying the missing decision rather than by changing its mechanism, which is
still the right one.

### The one real authentication in this platform, and its clock

Identity Platform is the only identity store (ADR 0019), and the only process
that talks to it is the console.

```
grep -n 'authenticatedAt' frontend/portal/src/lib/server/identity.ts
grep -n 'SESSION_TTL_MS' frontend/portal/src/lib/server/session.ts
```

`authenticatedAt: now` is stamped at the instant `gipSignIn` returned — a real
authentication of a real person, the thing this platform otherwise does not
have. But `SESSION_TTL_MS = 12 * 60 * 60 * 1000` and nothing re-stamps the
field, so a sealed session asserts an authentication that may be eleven hours
old, and a fifteen-minute gate fed from it is satisfiable only in the first
quarter-hour after a sign-in. There is no step-up: no re-authentication
ceremony exists anywhere in the portal
(`grep -rni 'reauth\|step-up' frontend/portal/src` prints nothing).

So the honest statement of the instant problem is not "there is no
authentication". It is: **there is exactly one, it happens in a process that
holds no authority, and its clock is twelve hours wide.**

## Decision

### 1. A per-person operator identity is a per-person credential minted in the composition root, and its subject is a person

`qip-api` gains one more credential source, beside the four role tokens and
governed by the same rules:

- **`QIP_OPERATOR_CREDENTIALS_DIR`** names a mounted directory. Each *file* in
  it is one person: the **file name is the subject**, the **contents are the
  token**. Read once at start-up, in `main.rs` and nowhere else, into the same
  `Vec<Credential>` the role loop builds, at `Role::Operator`.
- Entries are collected through a `BTreeMap` so the credential list is in a
  fixed order whatever `read_dir` returns. A replay that reorders is not a
  replay, and an authentication list whose order depends on a filesystem is a
  list whose comparison order depends on a filesystem.
- The subject is the file name, refused unless it matches
  `[a-z0-9][a-z0-9._-]{1,62}`, and refused outright if it ends in `@env` — the
  namespace the deployment credentials occupy, borrowed from ADR 0042
  decision 4's refusal list for the same reason: a person who can be named
  `operator@env` is a person who can be mistaken for the deployment.
- The token keeps the existing floor: shorter than 32 characters is refused with
  the existing message. Duplicate subjects are refused. A named directory that
  is empty or unreadable **stops the process** rather than starting with no
  operator, in the direction the credential loop's own refusal already takes —
  `"no credential is configured; set at least QIP_TOKEN_OPERATOR. An API that
  would otherwise be unauthenticated does not start."` That message is itself a
  thing the implementer must change: after decision 1 it names, as the remedy, a
  variable a deployment holding the directory is forbidden to set, and a refusal
  that instructs an operator to do the thing that will refuse them next is worse
  than a generic one.
- **A deployment may not hold both doors.** If `QIP_OPERATOR_CREDENTIALS_DIR`
  is set and `QIP_TOKEN_OPERATOR` (or its `_FILE` spelling) is also set, the
  process refuses to start, naming both. Two doors into one role means the
  shared one silently re-admits `operator@env` alongside the named people, and
  nobody reading a log afterwards can tell which door an act came through. This
  is the shape `main.rs` already uses for `RETIRED_CREDENTIAL_VARIABLES`, both
  spellings, and for the same reason.

**A directory rather than the alternatives, and the alternatives are not
strawmen.** One variable per person (`QIP_TOKEN_OPERATOR_JANE`) puts the subject
in an environment variable name, which bounds it to an upper-case shell
identifier and makes the log's subject a mangling of the person's name. A single
JSON document mapping subjects to tokens puts several people's secrets in one
blob, so rotating one person rewrites everybody's secret and a diff of the file
is a diff of every credential. A directory is what the Secret Manager CSI driver
and Cloud Run's secret volumes already project (ADR 0024), one secret per file,
one person per secret, and adding a person is one reviewed Terraform entry.

**The method string stays `"api-bearer-token"`.** It is a bearer token. Naming
it anything else in the record an incident review reads would be the fixtures'
mistake made in production.

### 2. The authentication instant is still not fabricated, and this record adds no second `Presence` variant

A per-person bearer token is still a standing secret. Possession of one at
09:05 is evidence that somebody held it, not that the person named on it
authenticated. `Presence` keeps its single variant, `authentication_instant`
keeps refusing, and **all seven routes in ADR 0065's table stay refused after
decision 1 lands**.

This is the sentence most likely to be dropped in a summary, so it is stated
flatly: **decision 1 does not reopen a single signature-gated route.** A lane
that implements it and reports "Gate A closed" has reported a false statement
about the system. What decision 1 does is narrow the remaining problem from two
blockers to one, and make every shape in decision 4 possible; what it does not
do is produce an authentication.

Adding a second `Presence` variant fed from a per-person token would be ADR
0065's defect restored with a better-looking name: an instant the holder of a
secret asserts about themselves. The refusal stands until something checks a
person.

### 3. The console stays `viewer`, and this is a refusal rather than an omission

The cheapest way to make ADR 0042's design reachable is to mount the operator
token into the console. **Refused.** ADR 0018 bought a stated property with that
entitlement — a console compromise is a disclosure and not a control — and the
console is the process with the largest attack surface in this platform: a
public-facing Next.js application with a session layer, a sign-up flow, an
identity integration and a browser on the other end. Moving the platform's
halt, its eligibility decisions and its capital signatures behind that surface,
to obtain an audit fact, trades a control for a label.

It would also be unrecoverable in the direction that matters: `DELETE
/kill-switch` and `POST /kill-switch` are both `Role::Operator`, so a console
holding the operator token is a console that can halt the platform and clear
the halt, and the fail-closed argument in `auth.rs` — that the brute-force
budget must never stand between an operator and a halt — would then be an
argument about the console's availability.

### 4. Gate A's remaining half needs a credential this workspace cannot issue, and here is exactly what it needs

Three sub-gates, in order, each with its own record. **None of them is
authorised by this one.**

**A1 — a step-up in the console. No dependency; console-side only.** Before an
act that a freshness gate will judge, the console re-verifies the person's
credential with Identity Platform (`gipSignIn` today, a passkey ceremony under
ADR 0038 if that record is ever accepted) and stamps `authenticated_at` from
*that* check, never from the twelve-hour cookie claim. This is the difference
between an instant that means "they signed in at some point today" and one that
means what the fifteen-minute window was written to mean. It is buildable today
and it is useless on its own, because of A2.

**A2 — a way for console-verified presence to reach a request whose authority
the console does not hold.** Three shapes, and the decision between them is not
this record's because two of the three are infrastructure:

- **(a) Give the console operator authority.** Refused in decision 3.
- **(b) A two-part ceremony.** The console mints ADR 0042's assertion — bound to
  method, path, body hash, nonce and the stepped-up `authenticated_at` — and
  displays it; the operator presents it beside *their own* per-person token from
  their own client. It needs no dependency and no new primitive, and it is the
  fallback if a desk needs the four single-approver routes back before (c)
  exists. It is not recommended, for a reason that is easy to miss and fatal to
  discover later: **it joins two identity namespaces with a table nobody
  keeps.** The assertion's subject is the console's `gip:<localId>`; the
  credential's subject is the file name in decision 1. If they disagree the API
  must either refuse (and every deployment must maintain the mapping by hand,
  unenforced) or believe one of them (and the other is decoration). If (b) is
  ever built, the rule that makes it honest is that the per-person credential's
  subject **is** the identity provider's subject, and a mismatch refuses.
- **(c) Terminate identity in front of `qip-api`.** ADR 0013's own reversal
  condition names this as "the standard Google answer" and says that if
  verification is terminated by a gateway that forwards a trusted assertion,
  "the platform never parses a JWT" and ADR 0013 "should be reduced to nothing".
  This is the recommended direction. It is an infrastructure decision — what
  terminates, how nothing can reach the service around it, and what the
  forwarded assertion is trusted on — and it needs its own record with a plan,
  under the infrastructure rules, not a sentence here.

**A3 — the two-person rule, which none of the above closes.** This is the part
that leaves the two-dependency rule, and the argument is short.

Every mechanism above authenticates through, or is signed by, **one key held by
one process**. Under ADR 0042's design the assertion key is a single Secret
Manager slot mounted to two workloads. Whoever can read that slot can mint two
assertions naming two different subjects with two different instants, and the
kernel's distinctness check — which compares strings — cannot tell them apart.
A two-person rule whose forgery cost is "read one secret this deployment
mounts" is a control that reads as protection against a single insider and is
not, and the population who can read that secret is the population who approve.
That is the `MaxExpectedShortfall` shape the risk rules name by example,
arriving one layer up, exactly as ADR 0043 predicted for `ed25519-dalek`.

What a two-person rule actually needs is **per-person key material the platform
verifies and cannot itself mint** — an asymmetric signature from a credential
that lives with the person (a passkey, a hardware token, a per-person KMS key),
verified by the API. That is ADR 0043's gap 1, named there as one of three gaps
no in-tree code may close, and its verification half is the one place ADR 0043
says a small audited crate is the right answer. **This record does not
authorise that crate, does not name one, and does not authorise the KMS path
either.** It states the requirement so that the next design does not discover it
at implementation time:

> A dual-signature route in this platform is honest only when the two
> signatures are produced by two credentials the platform cannot produce. That
> needs either a reviewed signature-verification dependency admitted under ADR
> 0012's three-part test, in its own ADR, or an identity terminator outside the
> process that performs the verification. Both are external components. Neither
> is `serde`.

**So: Gate A cannot be completed within the two-dependency rule.** Its subject
half can (decision 1). Its instant half can be *made real* only by A1 plus A2,
of which A1 needs nothing and A2's recommended shape needs infrastructure rather
than a crate. Its two-person half cannot be closed by any code this workspace is
permitted to contain, and pretending otherwise would ship the fifth instance of
this repository's own documented failure shape.

### 5. Nothing here is a second door into authority

Decision 1 adds credentials at a role that already exists; it adds no role, no
capability and no route. The role a caller holds still comes from the credential
they present and from nowhere else, and `Principal::require` is unchanged.

`AutonomyController::request_change` is named explicitly because it is the
thing an operator-identity change must never touch: it takes an operator
identity, holds it to `MAXIMUM_CLEARANCE_CREDENTIAL_AGE` and requires a second
approver, and after decision 1 it still refuses every caller, because
`authentication_instant` still refuses. **An autonomy escalation is not made one
step easier by anything in this record, and the mechanism that keeps it that way
is a refusal that is already in the tree rather than a new check this record
adds.** If a later lane finds itself widening `Presence` to make an autonomy
change pass, that is this record being cited for the one thing it forbids.

## The design handed to the implementing lane

Small enough to state completely.

### Where it sits

`backend/crates/apps/qip-api/src/main.rs` — the credential loop — and
`backend/crates/apps/qip-api/src/auth.rs` for a pure function that turns a
directory listing into `Vec<Credential>`. `infrastructure/terraform/` gains one
secret per person and one mount, in the shape `catalogue.tf`'s `secret_mounts`
block already uses. Nothing else.

### The dependency-direction argument

- `qip-api` is an **app**. It reads the environment and the filesystem, which is
  what a composition root is for; the rule is "configuration is read here and
  only here", and this is configuration.
- The parsing half is a pure function of `(entries, now) -> Result<Vec<Credential>>`
  living in `auth.rs`, so the decision is exercisable without a directory —
  the shape `qip_core::secret::resolve_from` and ADR 0042 decision 10 already
  establish. **The I/O stays in `main.rs`; the judgement is testable.**
- `Credential::from_token` is called with a different subject string. `qip-core`
  (**lib**) learns nothing, `qip-risk-engine` (**service**) learns nothing, the
  kernel (**runtime**) learns nothing: `OperatorIdentity::verified` is called at
  the same seven sites with the same third argument, which still comes from
  `authentication_instant` and still refuses.
- **No lib gains I/O. No lib depends on a service. No service depends on the
  runtime. Nothing depends on an app** — the credential list is app-private and
  passed downward as a value.
- **No new dependency.** No crate, no npm package, no primitive. `serde` and
  `serde_json` remain the whole of it, and `scripts/check-dependencies.sh` has
  nothing new to say.

### What it must refuse, and the admission that proves each gate is not refusing everything

A gate proven only by its refusal is a gate nobody has shown can admit. Each row
needs both halves, and the second column is the half that usually goes missing.

| Refusal | The admission beside it |
|---|---|
| a subject outside `[a-z0-9][a-z0-9._-]{1,62}` | `a.duarte` mints a credential |
| a subject ending `@env` | `operator.jane` mints one |
| two files naming the same subject | two files naming two subjects mint two |
| a token under 32 characters | a 32-character token mints one |
| an empty or unreadable directory | a directory with one valid file starts the process |
| both `QIP_OPERATOR_CREDENTIALS_DIR` and `QIP_TOKEN_OPERATOR` set | either one alone starts the process |

### The two tests an implementer would not think to write

- **A per-person credential still cannot pass a freshness gate.** Present a
  per-person operator token to `POST /strategies/:strategy/promotion-approvals`
  and assert the 403 and the refusal text from `authentication_instant`. This is
  the test that stops decision 1 being reported as Gate A. Mutate it by adding a
  `Presence` variant that returns `Some(issued_at)`; it must fail.
- **Two people are two subjects at the kernel, and one person is one.** Drive
  `Platform::approve_promotion` with two distinct subjects and assert the
  promotion completes; drive it with one subject twice and assert the refusal
  names "a second session is not a second person". Both against the kernel with
  explicit instants, which is what a test may do and a composition root may not.
  This asserts the thing decision 1 is *for*, in the only place it is currently
  reachable.

Mutation-verify both, and say which mutation fired.

## What it costs

- **Decision 1 opens no route, and the platform will look unchanged.** Four
  routes that ADR 0065 closed stay closed; three that were never completable
  stay incompletable. The only observable difference is in the log: `POST
  /kill-switch` journals `api:{principal.subject}` (`routes.rs`, and the console
  gateway's trip path in `console.rs`), so a halt starts naming a person instead
  of a role. That is one fact in one record, and it is the whole of today's
  yield. Anyone measuring this change by capability will conclude it did
  nothing.
- **Revoking a person becomes a deployment.** A file per person in a mounted
  directory means removing somebody is a Terraform change and a new revision,
  not a click. On the day somebody leaves the desk, their token works until the
  next deploy. The alternative — a runtime-editable operator list — is a control
  plane an incident can edit, which is worse; but the latency is real and a
  runbook has to say the rotation is the fast path.
- **Credential expiry keeps the shape it has**, which is thirty days measured
  from process start (`now.saturating_add(Duration::from_days(30))` in the
  credential loop). For a long-running process that is thirty days after boot,
  not thirty days after issue, and per-person credentials inherit it unchanged.
  This record does not fix that; it names it so the next reader does not take
  the per-person mount for a rotation policy.
- **More people hold operator tokens than before.** Today one secret is shared;
  afterwards each person has their own, which is the point, and it is also more
  secrets in more places. The blast radius of each is smaller and the number of
  radii is larger.
- **Gate A stays open, and ADR 0075's capital route stays refused in fact.**
  This record does not move Gate B or Gate C one step, and a reader who wanted
  capital issuance gets from it a clearer statement of why they cannot have it.
- **The honest answer to "when will a desk be able to approve a promotion?" is
  "after an external component is chosen, in its own record".** That is a worse
  answer than a date and a better one than a date nobody could meet.
- **ADR 0042 carries a correction.** Its mechanism survives; its route set does
  not work in the deployment it describes. Somebody has to read this record
  before implementing that one, which is one more edge in the register.

## What would make this wrong

- **A second `Presence` variant fed by anything the credential's own holder
  asserts.** The refusal in decision 2 is the whole of ADR 0065 held. If a
  variant appears whose `attested_at()` returns a number derived from a bearer
  token, a header nobody verified, or the request clock, this record is void and
  ADR 0065's defect is back with a better name.
- **A dual-signature route admitted on two subjects minted under one key.**
  Decision 4's A3. If the promotion, recalibration or reinstatement routes are
  ever opened on assertions a single secret can produce, the two-person rule has
  become a one-person rule that reads as a two-person rule, and the correct
  response is to close them again rather than to document the caveat.
- **The console acquiring the operator token.** Decision 3. If it happens, ADR
  0018's stated property is gone and the record that removed it should be the
  one that says so.
- **A signature-verification crate arriving in the lockfile without its own
  ADR.** Decision 4 names the requirement and authorises nothing. A crate that
  appears citing this record has been cited for the opposite of what it says —
  the failure mode ADR 0043 predicted for itself and wrote a scope paragraph
  against.
- **A per-person subject reaching `AutonomyController::request_change` and
  making an escalation possible.** Decision 5. Operator identity is about who
  may approve a capital grant; if it ever eases an autonomy change or an order
  path, this record was implemented as something other than what it says.
- **Evidence that refusing the seven routes is driving recovery through an
  unaudited path.** ADR 0065's third reversal condition, inherited whole: if the
  desk starts editing kernel state because the API refuses, the refusal has
  moved the risk rather than removed it, and the answer is A1 plus A2 rather
  than a widened gate.
- **Two identity namespaces in one record.** If a deployment ends up with a
  per-person token subject and a console subject for the same person and the log
  carries both, the join this record exists to avoid has been recreated inside
  it. Decision 4's rule for shape (b) is what prevents it.

## Consequences

- ADR 0075's Gate A is **partly decided and openly incomplete**: the subject is
  decided and buildable, the instant is specified and needs A1 plus A2, the
  two-person rule is refused pending an external component. Gates B and C are
  untouched, so the capital-issuance route stays authorised in shape and refused
  in fact.
- ADR 0042 is amended by the addition of the carrier decision, not by a change
  to its mechanism. Its decision 6 route set should be read together with
  decision 4 here before anybody implements it.
- `set_baseline` is unaffected and stays uncalled
  (`grep -rn --include=*.rs 'set_baseline' backend/crates` prints one line, the
  definition in `central/factory.rs`). It is gated on a promotion that is gated
  on Gate A, so it is downstream of this record twice over, and a lane that
  wires it before Gate A closes will have wired a writer nothing can reach.

## What is still the owner's

1. **Whether decision 1 is worth building before A2 exists.** It opens no route.
   The argument for building it now is that it is a prerequisite of every shape
   in decision 4, it is small, and it makes a halt attributable today. The
   argument against is that it is a control-plane change delivering one log
   field, and a desk that would rather wait for the whole path is being
   reasonable.
2. **Which A2 shape.** (c) is recommended and is an infrastructure decision with
   a plan to read; (b) is the no-dependency fallback with a named defect. Nobody
   should take (a).
3. **Whether the desk wants a two-person rule at all at this scale.** It is
   worth asking out loud rather than assuming. `ApprovalChain` also has a
   single-approver path below its dual threshold, and a desk of three people may
   prefer one named approver who cannot approve their own request — which the
   chain already refuses — over a second signature that only a shared secret
   makes possible.

## Gates run for this record

**None, and the reason is not that they were skipped.** No Rust, TOML,
Terraform or TypeScript file is changed by this record, so `cargo fmt`,
`cargo clippy`, `cargo test`, `terraform validate` and the frontend gates have
nothing to judge. The gate with a bearing is the documentation acceptance suite,
which requires every record to state what it costs and to be listed in the
index; both hold by construction here, and the run is named in the handover
rather than claimed. `./scripts/check-dependencies.sh` is untouched by a record
that admits no dependency, and `./scripts/check-secrets.sh` matters because this
document quotes variable names and file paths from the credential path — every
quotation above is a variable name, a constant, a message or a path, and no
token, key or digest of one appears in it, but that is this author's reading and
not a scanner's finding.

## The paper-trading boundary

Untouched, and confirmed rather than assumed. The three layers stand exactly as
ADR 0003 and ADR 0021 left them: Terraform's refusal of `supervised_live`,
`limited_autonomous_live` and `autonomous_live` at plan time in
`infrastructure/terraform/variables.tf`; `AutonomyLevel::deployable` refusing
the same three at start-up in `qip-api`, `qip-fastbrain` and `qip-deepbrain`;
and the type system — `qip-edge`'s `Cell` having no constructor taking a ceiling
other than paper trading, and `qip-cost-router`'s `Determinism::Required` arm
returning a type that cannot name a model rung.

Nothing here creates, enables or eases an order path. This record decides who
may be *named* as having approved something, and in its central half decides
that nobody may yet be named as the second person. It adds no capability,
removes no refusal, changes no default, and leaves every route that was refused
before it refused after it.
