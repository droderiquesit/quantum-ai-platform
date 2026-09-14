# ADR 0065: A standing secret has no authentication instant, and a freshness gate refuses one

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0024 (secrets reach a
  process as files), ADR 0061 (rule regret and the signed file that moves a
  bound), ADR 0062 (a venue is reinstated only by two signatures, and its
  Amendment B)

## Context

Seven places in this workspace ask whether the human who authorised something
was recently present:

| Gate | Window |
|---|---|
| `Platform::decide_eligibility`, `decide_investment` | `ELIGIBILITY_CREDENTIAL_AGE` |
| `Platform::approve_registration` | `REGISTRATION_CREDENTIAL_AGE` |
| `Platform::approve_promotion`, `approve_recalibration`, `reinstate_venue` | the promotion window |
| `KillSwitch::clear_global`, `AutonomyController::request_change` | `MAXIMUM_CLEARANCE_CREDENTIAL_AGE` |
| `ApprovalChain::grant` | `qip_compliance::approval::MAXIMUM_CREDENTIAL_AGE` |

All are fifteen minutes, all read `OperatorIdentity::is_fresh` or its twin on
`OperatorCredential`, and every one of them documents the same sentence:
*a session token from this morning is not evidence that anyone is at the
keyboard now.*

There is exactly one production site that mints the identity those gates
judge: `qip-api/src/routes.rs`, seven call sites, each building an
`OperatorIdentity` from the authenticated `Principal`. The instant it supplied
was `Principal::issued_at`.

**`Principal::issued_at` was the process's start-up instant.**
`qip-api/src/main.rs::run` opens with `let now = clock.now();`, and the loop
that reads `QIP_TOKEN_MONITOR`, `QIP_TOKEN_VIEWER`, `QIP_TOKEN_ANALYST` and
`QIP_TOKEN_OPERATOR` stamps every `Credential` it mints with that one value.
The tokens are standing secrets: static values in Secret Manager, mounted as
files (ADR 0024), rotated on a human schedule. So for the life of a process,
every caller of every signature-gated route carried the same `issued_at`, and
the fifteen-minute window measured **process uptime**.

Two consequences, both real and both shipped:

- **A stale token was admitted.** A process restarted at 09:00; a six-week-old
  copy of `QIP_TOKEN_OPERATOR` — from a laptop backup, a CI log, a
  shoulder-surfed `curl` — presented at 09:05 computed an age of five minutes
  and passed every gate in the table.
- **A legitimate operator was refused, permanently.** From 09:15 onward the
  same routes refused every caller, and no re-authentication helped, because
  there is nothing in this platform to re-authenticate against. Only a restart
  reopened the window, which means the documented remedy — "re-authenticate" —
  was a sentence naming an action nobody could take.

This is the shape `.claude/rules/domains/risk-and-execution.md` names by
example: `MaxExpectedShortfall`, a control that reads as protection, passes its
tests, and cannot do what it says. It is the fifth instance found in this
engagement.

**The tests did not catch it because they could not.**
`qip-acceptance/tests/compliance_proof.rs::the_two_credential_windows_that_
claim_to_be_the_same_window_agree_on_the_same_credential` constructs both
credentials with a synthetic `authenticated_at` and proves the two *windows*
agree — a true and useful property about two constants, and one that says
nothing about what the binary puts in the field.
`security.rs::every_operatoridentity_is_built_from_the_principals_durable_
subject_not_a_session_value` walks the *first* argument of every
`OperatorIdentity::verified` call in `routes.rs` and holds it to
`principal.subject.clone()`. Nothing held the third. Every route-level test in
`qip-api/tests` minted its credential at `start()` and called at `start()`,
where a correct implementation and this one agree exactly.

There is an earlier layer to this. The registration route once passed `now`,
making the window a comparison of a value with itself; that was found, fixed,
and guarded by a test asserting both halves of a window. The fix replaced one
fabricated instant with another, and the new test proved the fabrication
worked. A gate whose input is invented cannot be repaired by inventing it
differently.

## Decision

**A credential minted from a standing secret carries no authentication
instant, the composition root stops fabricating one, and a freshness gate
handed nothing refuses.**

Three changes, all in `qip-api`:

1. `Principal::issued_at` is documented for what it is — the instant *this
   process* minted its record of the token — and is no longer reachable as an
   authentication instant by anything that matters.
2. A new type states the absence: `qip_api::auth::Presence`, with one variant,
   `Unattested { credential_minted_at }`. Its `attested_at()` returns
   `Option<Timestamp>`, and it is `None`. One variant is the finding, not an
   unfinished enum: every credential class this API accepts is a standing
   bearer token, and possession of one is not presence. A class that genuinely
   carried an instant would be a second variant, and that is the shape a real
   fix takes.
3. `Principal::authentication_instant(action) -> Result<Timestamp>` is the one
   seam a route may ask through, and it refuses — naming the act, naming the
   credential's minting instant so a reader can see what number they might
   have mistaken for an authentication, and naming what would be required
   instead.

The refusal is at the route rather than in the kernel because the credential
class is the API's own fact. A kernel gate can only judge the number it is
handed; the composition root is the only place that knows the number is not a
fact about a person. `Option` rather than a sentinel, and a `Result` the route
must destructure rather than a boolean it might ignore: a sentinel — epoch, or
the start-up instant — is precisely what the defect looked like from the
reading side.

HTTP answer: **403**. The caller is authenticated and this deployment can never
authorise the act; nothing about the request or the platform's state would
change the answer, so 409 — which this API uses for a refusal on state — would
invite a retry that cannot succeed.

## What it costs

### What the platform now refuses

Seven routes refuse every caller, consistently, instead of admitting one inside
a window keyed to pod age:

`POST /ledger/users/:user/eligibility`,
`POST /ledger/users/:user/investment-requests`,
`POST /strategies/:strategy/promotion-approvals`,
`POST /risk/recalibrations/:rule/approvals`,
`POST /venues/:venue/reinstatements`,
`POST /registrations/:source/approve`,
`DELETE /kill-switch`.

### The functional cost, stated honestly

It is not uniform, and flattening it would be the same kind of claim this ADR
exists to correct.

**Three of the seven were already impossible for their stated purpose.** The
promotion, recalibration and venue-reinstatement signatures each require two
*different* people, and the kernel compares `OperatorIdentity::subject()`. The
composition root mints the operator credential per **role**, not per person:
`format!("{}@env", role.as_str())`. Both humans holding `QIP_TOKEN_OPERATOR`
present `operator@env`, so the countersignature was refused as one person
signing twice however many sessions were opened. ADR 0062 Amendment B records
that finding. No dual-signature route could be completed by two people before
this change, and none can after it; what this change removes on those three is
the fifteen-minute window in which a stale copy of the token could place the
*first* signature.

**Four genuinely worked, inside the uptime window, and now do not.** The
registration approval, the two ledger decisions and the kill-switch clearance
each need one operator. Each was usable for fifteen minutes after a restart and
unusable thereafter. So the capability lost is real but was already keyed to a
number — the pod's age — that no operator could reason about or observe, and it
was available on exactly the same terms to anyone holding any copy of the
token, of any age.

**The kill switch deserves its own sentence.** Tripping it is unchanged and
needs no authority; only clearing refuses. That is the fail-closed direction of
a control whose safe state is "stopped". The halt is not resumed from the event
log, so restarting the process still lifts it — a deployment action with its
own audit trail. What is lost is the `KillSwitchClearance` record naming who
lifted it, which was obtainable only during the first fifteen minutes of a
process's life. `docs/operations/kill-switch.md` describes the API route and is
now describing a path that refuses; correcting it is outside this lane's
territory and is named in the handoff.

**Two behaviours lose their only caller.** `Api::readmit_connector`, which
re-runs the licensing gate on the record an approval just wrote, and the
connector-admission verdict it produces, are reached only from the approval
route's success path. They are kept rather than deleted: they are what a real
fix restores, and deleting them would make the restoration a rewrite.

### What a real fix needs, and what this is not

This ADR does not build an authentication system and should not be read as
deferring one indefinitely. Either of two things closes it:

- **A per-request proof of recency.** The caller signs a challenge or a current
  timestamp with a key the platform can verify, and the *signature's* instant —
  not the credential's — is what `is_fresh` judges. This needs an asymmetric
  primitive, which ADR 0043 names as one of three gaps no in-tree code may
  close, so it needs a reviewed dependency and its own ADR first.
- **An interactive authentication step.** A human-facing sign-in whose session
  carries a real `authenticated_at` and a *per-person* subject. That second
  half matters at least as much: it is what would make a dual signature
  obtainable at all, and without it a freshness fix restores three routes to
  being refused for a different reason.

Until one of those exists, the acts above are taken by someone with direct
access to the kernel or by a deployment change, and this ADR is the record that
they are taken that way on purpose rather than by oversight.

### On refusing rather than lowering

The alternative considered and rejected was to keep admitting inside the
window, on the ground that a working-if-flawed control beats an absent one.
It does not, here. The window admitted a credential of unbounded age; an
operator reading "refused after fifteen minutes" would reasonably conclude the
platform tracked their presence, which is the false belief a control is
supposed to prevent rather than create. A refusal that names what would be
required is a smaller lie than a gate that measures the wrong thing and reports
a number.

### Tests

Every route-level test that asserted one of these acts succeeding asserted a
property of its fixture. Those tests are rewritten to assert the refusal, and
where the substance was the *kernel's* behaviour — the mandate's arithmetic,
the registry's record, the countersignature comparison — it is now asserted
against the kernel, with an operator identity the test constructs and an
explicit instant, which is exactly the thing a test may do and a composition
root may not. The two acceptance assertions named in Context are corrected in
place with the reason written beside them.

## What would make this wrong

Three things, and the first two are fixes rather than refutations.

**A credential class that does carry an authentication instant.** If an
interactive sign-in lands, `Presence` gains a second variant, `attested_at()`
returns `Some` for it, and every gate in the table above starts judging a
number that means something. Nothing else here changes: the refusal is written
as the answer for *this* class, not as a claim that presence is unknowable.

**A per-request proof of recency.** Same outcome by a different route, and the
better one for a machine-to-machine surface. It needs an asymmetric primitive
and therefore an ADR of its own; see ADR 0043 for why no in-tree code may
supply it.

**Evidence that refusing these seven routes causes recovery to happen by an
unaudited path.** This is the argument `qip-api/src/venue_views.rs` makes
about the reinstatement route itself: a safety control that cannot be recovered
from through the platform's own audited path invites recovery through one
nobody audited. If the desk starts editing state directly because the API
refuses, the refusal has moved the risk rather than removed it, and the answer
is to build one of the two mechanisms above rather than to widen the gate. It
is not to restore a window measured in pod uptime, which was never an audited
path either — it was the same unaudited access with a fifteen-minute timer on
it.

What would **not** make this wrong: discovering that an operator wants one of
these routes back. The routes were never keyed to that operator's presence, and
a control that measures the wrong thing is not made right by somebody needing
it.
