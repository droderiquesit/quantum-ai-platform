# Ledger, wallet, corridor and transfer-gate routes

The read-only treasury surface of `qip-api`. Four `GET` routes under
`/api/v1`, all answering `200` with `content-type: application/json`.
`/ledger/users` requires the `analyst` role; `/wallet`, `/corridors` and
`/transfer-gate` require `viewer`. The split is by what the body carries:
`/ledger/users` lists every user in the mandate registry with their mandate,
balances and inflow references, and the portal grants `viewer` to anyone who
completes self-registration, so at `viewer` the route would hand every user's
capital to whoever could sign up. The other three describe the process — the
wallet the kernel's fabric journal last assembled, the registries it holds,
the gate's checks and newest assessment, and the kill switch — and carry no
per-user datum. `POST`, `PUT`, `PATCH` and `DELETE` on any of them answer
`405 {"error":"that method is not allowed here"}`. Nothing here submits,
approves, signs or moves anything, and there is no route that could: ADR 0021
permits the deterministic half of the blueprint's treasury and refuses the
path by which capital leaves the platform.

Beside the four reads are two `POST` routes at the `operator` role, both under
`/ledger/users/{user}`: `/eligibility`, which records an operator's decision
that a user may or may not have capital put to work, and
`/investment-requests`, which raises one request against a mandate and answers
the ledger's verdict. Both raise a typed intent into the kernel under the
authenticated operator's own identity, both are journalled before they are
answered, and neither moves capital: an eligibility record is a *precondition*
of a funding and an admitted investment request is a *statement* that the
mandate would admit one. The paragraph above said "four routes, read-only" and
listed no `POST` at all while the eligibility route had been live for some
time; that is corrected here rather than left, because a page built from this
file would not know the surface it is describing.

Since ADR 0085 §5 there is a fifth read and a third pair of operator writes,
and they are the desk's rather than a user's: `GET /ledger/commitments` lists
every unfunded private commitment the desk is on the hook for with the
capital-call notices standing against each, at `viewer` because it carries
no per-user datum, and `POST /ledger/commitments/{commitment}/capital-calls`
with its `DELETE` by reference file and withdraw a fund's drawdown notice at
`operator`. A notice can only ever *raise* what the reserve holds back; no
route settles one, and each section below says why.

Beside it, `GET /ledger/private-positions` at `viewer` is blueprint §40.1's
private-positions surface: the same commitments and notices, the mark the
valuation plane struck and the method it used, and the distributions the
administrator's record dates. It is the wider of the two — a holding wholly
called has a mark and no commitment — and it declares no write, because
§40.1's act on that row is to commit and to decline a call, and this platform
has no channel to say either thing to a fund.

Every body is read off the kernel at request time. The wallet, the corridors,
the destinations and the gate assessment come from the kernel's fabric
journal, whose every decision is also a record in the platform's event log;
the users and balances come from the per-user ledger, which the kernel books
from the centre's exact attribution. The API keeps no copy of any of it.

The Rust shapes are in `src/ledger_views.rs`. This file is the same contract in
prose, kept exact so a page can be built against it without reading Rust.

## Conventions

- Every body carries `"posture": "PAPER TRADING"` as its first key. Render it.
- Every body carries `"served_at"`, the instant the platform answered, in
  RFC 3339 UTC (`"2025-10-09T08:53:20.000Z"`).
- Every timestamp is RFC 3339 UTC or `null`.
- Every money figure is a **string** (`"1000000"`, `"-12.5"`), never a JSON
  number. The platform's `Decimal` renders as its exact text; do not parse it
  into a float to display it.
- Keys are stable `snake_case`. Lists are ordered deterministically (by user
  id, then strategy id, then currency code; by venue then asset; by corridor
  or destination key) so two reads of the same state render identically.
- Absence is stated, never zero-filled: where the platform does not yet hold a
  thing the body says so with a boolean and a `reason` string.

## `GET /api/v1/ledger/users`

**Role: `analyst`.** Every user in the per-user ledger's mandate registry —
the desk, and each user mandate the deployment's configuration enrolled under
it — with their mandate, their per-strategy balances and the entitlement
evaluation for the viewer role. A `viewer` credential answers `403`; the
entitlements in the body are still *evaluated as* the viewer role, which is a
property of the evaluation, not of who may read it.

Balances are what the kernel booked: a user's book at a strategy opens when
the user's mandate funds it, and every fill the centre settles is then split
across the users with capital at work at that strategy in proportion to what
each has there, exactly, with the rounding unit assigned to the largest
holder. With no user enrolled the desk takes each fill whole. Either way the
event log carries the booking and its basis.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "evaluated_as_role": "viewer",
  "products": ["research-tests"],
  "fills_journalled": 2,
  "users": [
    {
      "user_id": "alice",
      "mandate": {
        "capital": "1000",
        "currency": "USD",
        "risk_tolerance": "1",
        "liquidity_floor": "0",
        "investable": "1000",
        "exploration_share": "0",
        "jurisdiction": "GB",
        "permitted_families": { "any": true, "families": [] }
      },
      "balances": [
        {
          "strategy": "alpha",
          "currency": "USD",
          "settled": "433.333333333",
          "reserved": "0",
          "available": "433.333333333",
          "expected_inflows_total": "0",
          "expected_inflows": [],
          "entries": 2,
          "last_entry_at": "2025-10-09T08:53:20.000Z"
        }
      ],
      "entitlements": [
        {
          "family": "research-tests",
          "role": "viewer",
          "evaluated_at": "2025-10-09T08:53:20.000Z",
          "can_view": { "granted": true, "reason": "alice holds a mandate in GB" },
          "can_invest": { "granted": false, "reason": "alice holds the viewer role, which does not invest" },
          "can_withdraw": { "granted": false, "reason": "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision" }
        }
      ],
      "entitlements_note": null
    },
    { "user_id": "desk", "mandate": { "capital": "10000000", "...": "..." }, "eligibility": { "eligible": false, "verified_at": null, "can_invest": null, "jurisdiction": null, "expires_at": null, "refused": "unknown_user", "reason": "desk is not eligible (unknown_user): ..." }, "balances": [], "entitlements": [], "entitlements_note": null }
  ]
}
```

Field by field:

| Key | Type | Meaning |
|---|---|---|
| `evaluated_as_role` | `"viewer"` | The ledger role every entitlement was evaluated under. This surface is the viewer's; it never evaluates as an investor or the desk. |
| `products` | `string[]` | The strategy families registered with the central factory, which are the products an entitlement is evaluated against. Empty on a fresh platform. |
| `fills_journalled` | integer | Attributed fills the ledger has booked since assembly, whichever basis each was booked under. |
| `inflow_posting` | string | A constant sentence: no declared inflow is ever posted by this build, so every `expected_inflows` entry is a claim and never becomes a balance here (ADR 0085). Render it beside any expected inflow shown. |
| `inflow_refusal` | string or null | The refusal **this caller** would receive at `POST`/`DELETE` of an expected inflow, or `null` if they would get past the presence gate. Distinct from `inflow_posting`, which is a fact about the build: this is a fact about the reader, and it is `"…a standing bearer token cannot carry it…"` for every credential this deployment accepts (ADR 0065, ADR 0076). A page offering a declaration form renders this instead of attempting the write to discover it. |
| `users[].user_id` | string | The ledger's user id. `"desk"` is the platform's own book and is always present; the rest are the configuration's enrolments, in id order. |
| `users[].mandate.capital` | money string | Capital under management. |
| `users[].mandate.currency` | string | ISO 4217 code. |
| `users[].mandate.risk_tolerance` | decimal string in `[0, 1]` | Share of capital the user tolerates losing. |
| `users[].mandate.liquidity_floor` | money string | Capital that stays liquid however strategies are sized. |
| `users[].mandate.investable` | money string | `capital - liquidity_floor`. |
| `users[].mandate.exploration_share` | decimal string in `[0, 1]` | Share spendable on information gain. |
| `users[].mandate.jurisdiction` | 2-letter string | ISO 3166 alpha-2; `"ZZ"` is the desk's own. |
| `users[].mandate.permitted_families` | `{any: bool, families: string[]}` | `any: true` means every family; otherwise `families` lists the only ones. |
| `users[].eligibility` | `{eligible: bool, verified_at, can_invest, jurisdiction, expires_at, refused, reason}` | The ledger's own verdict at request time on whether this user may have capital put to work. When `eligible` is `true` the four terms are the record an operator wrote (timestamps, a bool, a 2-letter string) and `refused`/`reason` are `null`; when `false` the terms are `null`, `refused` is the ledger's stable token (`no_mandate`, `unknown_user`, `revoked`, `not_yet_verified`, `cannot_invest`, `expired`, `jurisdiction_absent`) and `reason` its sentence naming what to do. The desk reads `unknown_user` until an operator decides otherwise; no field about withdrawing exists on the record (ADR 0021). |
| `users[].balances[]` | list | One row per `(strategy, currency)` book the user holds. Empty until the user's mandate has funded a strategy or a fill has been attributed to the user. |
| `balances[].settled` | money string | Cash the ledger has said is here: funded, plus the user's exact share of every fill since. |
| `balances[].reserved` | money string | Settled cash held against an unresolved proposal. |
| `balances[].available` | money string | `settled - reserved`. Expected inflows are **not** in this figure, and nor is `uninvestable`. |
| `balances[].uninvestable` | money string | Cash that arrived past the mandate's investable or contribution ceiling when it was posted, held and never sized against (ADR 0085). `"0"` on every balance in this build, because nothing posts an arrival — see `inflow_posting`. |
| `balances[].expected_inflows_total` | money string | Sum of declared, unposted inflows. Reported so it is visible; never added to anything. |
| `balances[].expected_inflows[]` | `{reference, amount, declared_at}` | Each declared inflow by the reference the user supplied. |
| `balances[].entries` | integer | Attributed fills booked here. Distinguishes "none booked" from "balance happens to be zero". |
| `balances[].last_entry_at` | timestamp or `null` | |
| `users[].entitlements[]` | list | One evaluation per product in `products`. Empty when `products` is empty. |
| `entitlements[].can_view` / `can_invest` / `can_withdraw` | `{granted: bool, reason: string}` | `reason` is the basis of a grant or the input that refused. `can_withdraw.granted` is **always `false`**; the platform's type has no granted arm. |
| `users[].entitlements_note` | string or `null` | Set when `entitlements` is empty, saying why (no product registered). |

## `POST /api/v1/ledger/users/{user}/investment-requests`

**Role: `operator`.** Raise one investment request against `{user}`'s mandate.
This is the blueprint's `investment-api` intent and the only one it has: it
raises a request and it never raises an order.

**It funds nothing.** The body's `funded` key is a constant `false` and is
there to be rendered. An admitted request means the mandate *would* admit this
much at this strategy at the instant it was decided; capital moves through the
capital engine's allocation, which re-runs the eligibility and product gates
on its own because the books may have moved since.

Request body — exactly these five keys, all JSON strings. Any other key is
`400`, named by position and never quoted back:

```json
{
  "strategy": "momentum-eu",
  "family": "momentum",
  "currency": "USD",
  "amount": "25000.00",
  "reason": "the desk raised this on the client's written instruction of 2026-09-07"
}
```

- `amount` is a **string**, like every money figure here. A JSON number is
  refused rather than converted: `25000.10` is not a double, and an amount a
  parser rounded is not the amount anyone asked for.
- `family` must be the family the central factory has registered the strategy
  under. A request naming a different one is refused rather than re-labelled —
  otherwise a mandate permitting only one family could be satisfied by naming
  that family over a strategy in another. A strategy the factory does not know
  is refused too.
- The user is the path's, resolved against the mandate registry; a user with no
  mandate is `404`. The instant of the request is the server's clock, not the
  caller's.

Answer, `200`:

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "decided_at": "2025-10-09T08:53:20.000Z",
  "request": { "user_id": "alice", "strategy": "momentum-eu", "family": "momentum", "currency": "USD", "amount": "25000.00", "requested_at": "2025-10-09T08:53:20.000Z" },
  "admitted": false,
  "funded": false,
  "refused_limit": "InvestableCapital",
  "detail": "25000.00 USD exceeds the 10000 investable for alice: ...",
  "user": { "user_id": "alice", "mandate": { "...": "..." }, "eligibility": { "...": "..." }, "balances": [], "entitlements": [], "entitlements_note": null }
}
```

| Key | Type | Meaning |
|---|---|---|
| `request` | object | The request as the *ledger* recorded it, echoed from the decision rather than from what was posted. |
| `admitted` | bool | The mandate's verdict. A refusal is a `200` carrying `admitted: false`, not an error: the platform decided, and the decision is the answer. |
| `funded` | `false` | Constant. Names the property a page must render: nothing moved. |
| `refused_limit` | string or `null` | The ledger's own name for the gate that refused: `NoMandate`, `Eligibility`, `Entitlement`, `Currency`, `Amount`, `InvestableCapital`, `RiskTolerance`. `null` when admitted. Group on this; do not parse `detail`. |
| `detail` | string | The basis of an admission or the sentence of a refusal, naming what would have to change. |
| `user` | object | The same row `GET /ledger/users` renders for this user, so the figures the verdict was reached against are readable beside it. |

Every outcome — admitted or refused — is journalled to the platform's event
log under the producer `kernel/investment`, with the operator who raised it and
the stated reason. A refusal that left no trace would be indistinguishable from
a request nobody made.

## `POST /api/v1/ledger/users/{user}/expected-inflows`

**Role: `operator`.** Declare, on `{user}`'s behalf, that a deposit is on its
way: the strategy it is for, the reference the wire will carry, and the
amount. This is blueprint §40.12's "Add capital" flow as far as ADR 0085
lets it go — the `expected inflow` step — and no further.

**It receives, posts and invests nothing.** The amount is held beside the
balance in `expected_inflows` and is not in `available`. It stays there until
an operator cancels it: **no declared inflow is ever posted by this build**,
because nothing here can honestly say a user's wire landed (the only
custodian statement the platform observes is the desk's own wallet), and the
body's `inflow_posting` says so in as many words. A page must render that
sentence beside any expected inflow it shows.

Request body — exactly these three keys, all JSON strings. Any other key is
`400`, named by position and never quoted back; a caller writing against the
blueprint's flow will send `source`, `destination` or `settled`, and none of
them is read:

```json
{
  "strategy": "momentum-eu",
  "reference": "SWIFT-2026-09-19-0001",
  "amount": "25000.00"
}
```

- `amount` is a **string**, for the reason every money figure here is.
- The declaration is refused (`409`, the ledger's own sentence) for a user the
  eligibility registry does not admit, by the reason named; for a reference
  already outstanding at any of the user's books, because a wire reference
  names one wire; and for an amount that, with what the user has contributed
  and every declaration still outstanding, would pass the capital the mandate
  places under management. The declaration is the last instant a refusal
  reaches the person before the wire is sent, which is why the contribution
  ceiling is asked here.
- The user is the path's, resolved against the mandate registry; a user with
  no mandate is `404`. The instant is the server's clock.
- **Refused in fact today**, `403`, for want of an attested person: every
  credential this API accepts is a standing bearer token, and the route dates
  the operator by `Principal::authentication_instant`, which refuses one (ADR
  0065, ADR 0075). The route is authorised in shape; it acts once a
  per-person credential exists (ADR 0076).

Answer, `200`: the user's `/ledger/users` row read back from the ledger after
it adopted the declaration, with `posture`, `served_at` and `inflow_posting`
beside it. The declaration and the operator's subject are journalled to the
event log under the producer `kernel/ledger` **before** the ledger adopts it,
and rebuilt from the log at the next boot, so a declaration lives in the log
and nowhere else.

## `DELETE /api/v1/ledger/users/{user}/expected-inflows/{reference}`

**Role: `operator`.** Record that the declared deposit under `{reference}` is
not coming. No body. Refused (`409`) for a reference no book of the user's
expects, so a cancellation is always of a declaration that stood; `404` for a
user with no mandate; `403` for want of an attested person, exactly as the
declaration is. Nothing else on the book moves, and the reference is free to
be declared again for the wire that does come. Answer, `200`: the same row
the declaration answers with. Journalled under `kernel/ledger` before the
ledger drops it, and replayed in log order after the declaration it cancels.

## `GET /api/v1/ledger/commitments`

**Role: `viewer`.** Every unfunded private commitment the desk is on the hook
for — derived at assembly from each private-asset record in the universe —
with the capital-call notices standing against it, as the reserve reads them.
Carries no per-user datum: a commitment is the desk's obligation to a fund,
and a notice is what the fund demanded of the desk. `obligation_total` is the
figure the kernel's `deployable_capital` subtracts from free capital before
anything is sized, and it is the book's own sum (`CommitmentBook::
unfunded_total`), not one this layer adds up: each commitment's `obligation`
is its `unfunded` balance plus the `accrued_default_penalty` its overdue
notices have cost. A notice ahead of its due instant therefore changes no
figure here; one past it adds exactly its `penalty`.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "call_settlement": "no filed capital call is ever settled by this build: …",
  "obligation_total": "251000",
  "accrued_default_penalty": "1000",
  "commitments": [
    {
      "subject": "obj-FUND",
      "committed": "400000",
      "called": "150000",
      "unfunded": "250000",
      "accrued_default_penalty": "1000",
      "obligation": "251000",
      "known_at": "2025-10-09T08:53:20.000Z",
      "capital_calls": [
        {
          "reference": "call-1",
          "amount": "100000",
          "issued_at": "2025-10-09T08:53:20.000Z",
          "due_at": "2025-10-19T08:53:20.000Z",
          "consequence": { "kind": "interest", "annual_rate_bps": 3650 },
          "overdue": true,
          "days_late": 10,
          "penalty": "1000"
        }
      ]
    }
  ]
}
```

| Key | Type | Meaning |
|---|---|---|
| `call_settlement` | string | A constant sentence: no filed capital call is ever settled by this build. Meeting a call is capital leaving the desk, which only the custodian it was paid from can attest, and no statement the platform observes names a fund administrator (ADR 0085 §5). Render it beside any capital call shown. |
| `obligation_total` | money string | What the reserve holds back across the book: every commitment's `obligation`, summed by the book. |
| `accrued_default_penalty` | money string | The penalty part of that total, on its own, so a reserve that grew and a payment the desk missed do not read alike. |
| `commitments[].subject` | string | The private asset's object id, which is the commitment's key. |
| `commitments[].committed`, `called`, `unfunded` | money string | The record's promised and drawn capital, and their difference. A notice never moves any of the three. |
| `commitments[].obligation` | money string | `unfunded` plus this commitment's `accrued_default_penalty`. |
| `commitments[].known_at` | RFC 3339 | When the platform last updated the record the commitment derives from. |
| `capital_calls[].consequence` | object | `{"kind": "interest", "annual_rate_bps": N}`, `{"kind": "forfeiture", "fraction_bps": N}` or `{"kind": "acceleration"}` — what failing the call costs, as the notice stated it. |
| `capital_calls[].overdue`, `days_late`, `penalty` | bool, integer, money string | Whether the money was due and the notice knowable by `served_at`; whole days past due, floored; and what missing it has cost so far. Zero and `false` ahead of the due instant. |

## `GET /api/v1/ledger/private-positions`

**Role: `viewer`.** Blueprint §40.1's private-positions surface: every private
holding the universe holds a record for, with the mark the valuation plane
struck and the method it used, the refusal where it would not mark one, the
commitment and its notices where the holding still has an unfunded balance,
and the distributions the administrator's own record dates. Carries no
per-user datum, for the reason `/ledger/commitments` does not: a private
position is the desk's holding.

**It is walked from the marks and not from the commitment book**, and a page
built against this file should not re-derive it the other way. A holding that
has been wholly called has no commitment — the book records one only where
capital is still unfunded — and is not thereby less of a position. A surface
built off `/ledger/commitments` would omit every fully-drawn fund silently.

Every figure here is one the platform already acts on rather than one
computed for display: `Platform::sizing_confidence` narrows a thesis by the
same `confidence_now`, refuses outright on the same `unmarkable` reason and on
`stale`, and the liquidity ladder dates a holding's exit from the same
schedule.

**Nothing here is this platform's forecast.** A flow under `distributions` is
the administrator's own reported residual dated at the record's own lockup
end. The platform derives no return, no rate and no probability of its own.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "call_settlement": "no filed capital call is ever settled by this build: …",
  "positions": [
    {
      "subject": "obj-FUND",
      "mark": {
        "available": true,
        "value": "160000",
        "method": "last_round",
        "struck_confidence": 0.4,
        "confidence_now": 0.2,
        "as_of": "2025-04-12T08:53:20.000Z",
        "next_review": "2026-04-12T08:53:20.000Z",
        "stale": false
      },
      "commitment": { "subject": "obj-FUND", "unfunded": "250000", "…": "…" },
      "distributions": {
        "stated": true,
        "flows": [
          {
            "kind": "distribution",
            "due_at": "2030-12-30T00:00:00.000Z",
            "amount": "160000",
            "probability": 1.0
          }
        ]
      }
    }
  ]
}
```

| Key | Type | Meaning |
|---|---|---|
| `positions[].subject` | string | The private asset's object id. Positions are listed in that order, so a replay renders the same page. |
| `mark` | object or absent | Absent exactly when `unmarkable` is present. The two are one fact and never both. |
| `mark.value` | money string | What the plane marked the holding at. |
| `mark.method` | string | `quoted`, `matrix`, `comparables`, `discounted_cashflow`, `model`, `last_round` or `cost` — how the mark was arrived at. Render it beside the value: the same number on a last round and on a quoted market are different claims. |
| `mark.struck_confidence` | number | What the method carried on the day the mark was struck. A statistic, never money. |
| `mark.confidence_now` | number | That confidence decayed to `served_at` at the method's own half-life. This is the number the platform sizes against, and it is why the struck figure is beside it rather than instead of it. |
| `mark.as_of`, `next_review` | RFC 3339 | When the evidence was true, and when the mark falls due for refresh. |
| `mark.stale` | bool | The mark's own `is_stale` at `served_at`. A stale mark is one nothing may be sized into; the platform refuses rather than sizing at a reduced fraction of it. |
| `unmarkable` | object or absent | `{"available": false, "reason": "…"}` — the valuation plane's own refusal, word for word. A record with no residual, no net cost and no schedule cannot be marked, and the reason names which. |
| `commitment` | object or absent | The holding's row exactly as `/ledger/commitments` renders it, including `capital_calls`. Absent where nothing is unfunded. |
| `distributions.stated` | bool | Whether the record dates a schedule at all. `false` is the record saying nothing — no residual reported, or a lockup already run out — and is **not** the same claim as a schedule with no flows. Do not render the two alike. |
| `distributions.flows[].kind` | string | `capital_call`, `fee`, `distribution`, `coupon` or `principal`. The direction is the kind's; `amount` is a magnitude and is never signed. |
| `distributions.flows[].amount` | money string | The flow's magnitude as the record states it. |
| `distributions.flows[].probability` | number | The likelihood the flow occurs at all, as the record states it. A statistic. |

## `POST /api/v1/ledger/commitments/{commitment}/capital-calls`

**Role: `operator`.** File a fund's drawdown notice against `{commitment}`:
the fund's own notice reference, the amount demanded, the instant it falls
due, and the consequence of failing it. This is blueprint §43.2's
`CapitalCall` reaching the commitment book from outside a test, as ADR 0085
§5 designed it — its own record family, a demand *on* the desk and not a
user's deposit — and no further.

**It pays, settles and transfers nothing, and can only ever raise what the
platform holds back.** A notice moves neither the called nor the unfunded
balance; it changes nothing until it is overdue, and then adds its penalty
to the obligation the reserve subtracts. **No filed call is ever settled by
this build**: `Commitment::settle_call`, the one method that would move the
called balance, is reached by no route, because meeting a call is capital
leaving the desk and an operator asserting that it left would be a person's
claim about the custodian's fact — the shape ADR 0085 §2 refuses for an
arrival, in the direction that would loosen sizing. The body's
`call_settlement` says so in as many words.

Request body — exactly these four keys. Any other key is `400`, named by
position and never quoted back; a caller who reads a notice as something to
pay will send `paid`, `settled` or `source`, and none of them is read:

```json
{
  "reference": "FUND-CALL-2026-Q4-01",
  "amount": "100000.00",
  "due": "2026-10-19T00:00:00Z",
  "consequence": { "kind": "interest", "annual_rate_bps": 800 }
}
```

- `amount` is a **string**, for the reason every money figure here is.
- `due` is an RFC 3339 instant with its zone; an epoch number is refused,
  because it is a unit nobody stated. The *issued* instant is the server's
  clock, not the caller's.
- `consequence` is **required and never defaulted**: `kind` is `interest`
  (with `annual_rate_bps`), `forfeiture` (with `fraction_bps`) or
  `acceleration` (with nothing else). The basis points are a rate, not money,
  and are the one figure this route reads as a JSON number. The three arms
  behave differently in time — interest grows with lateness, forfeiture is
  taken once, acceleration moves every remaining call to today — and a
  notice filed without one would reserve against a penalty nobody stated.
- Refused with the book's own sentence for a commitment the universe does
  not hold (`404`), and (`400`) for an amount that with the notices already
  standing would pass the unfunded balance, a due instant before the
  notice's own, or a reference already standing — a second notice under one
  reference is either a restatement or a second draw, and guessing which
  would move the reserve with no record of which claim won. To amend a
  notice, withdraw it and file again.
- **Refused in fact today**, `403`, for want of an attested person: every
  credential this API accepts is a standing bearer token, and the route
  dates the operator by `Principal::authentication_instant`, which refuses
  one (ADR 0065, ADR 0075). The gate stands *before* the commitment is
  looked up, so the refusal is the same on a book that holds no commitment
  at all. The route is authorised in shape; it acts once a per-person
  credential exists (ADR 0076).

Answer, `200`: the commitment's `/ledger/commitments` row read back from the
book after it adopted the notice, with `posture`, `served_at` and
`call_settlement` beside it. The notice and the operator's subject are
journalled to the event log under the producer `kernel/capital-call`
**before** the book adopts it, and rebuilt from the log at the next boot
through the same gates, so a notice lives in the log and nowhere else. A log
holding a notice this boot's universe will not take — a commitment dropped
from the catalogue, an unfunded balance the administrator's record has since
reduced below the notices standing — **stops assembly**, naming the record
and the remedy (withdraw the notice on the running process before deploying
the universe that invalidates it, or archive the log); skipping it would be
the erasure the record exists to end.

## `DELETE /api/v1/ledger/commitments/{commitment}/capital-calls/{reference}`

**Role: `operator`.** Withdraw the notice under `{reference}` — the fund
rescinded it, or it was filed in error. No body. Refused for a commitment
the book does not hold (`404`) and for a reference no notice stands under
(`400`), so a withdrawal is always of a notice that stood; `403` for want of
an attested person, exactly as the filing is. The called and unfunded
balances do not move — a withdrawal meets nothing — and the most it can do
to the reserve is stop charging the penalty the notice, had it stood, would
have accrued. Answer, `200`: the same row the filing answers with.
Journalled under `kernel/capital-call` before the book drops it, and
replayed in log order after the notice it retracts, so a reference filed,
withdrawn and filed again resumes filed once.

## `GET /api/v1/wallet`

The wallet the kernel's fabric journal last assembled, and its reconciliation
outcomes. The kernel observes no custodian, venue balance or chain address of
its own; it assembles a wallet in the LEARN stage of each cycle from the
statements handed to it (provenance `statement`, the one channel the process
can attest), pairing each with the ledger's view where the ledger books that
venue-asset — the desk's cash at its venue, with reservations against it —
and reconciling each against the tolerance supplied with the statement. Until
a statement has been handed in and a cycle has run, the body reports that no
wallet is assembled and fabricates no holding.

A statement reaches the kernel through the composition root: `QIP_WALLET_STATEMENT_PATH`
names a JSON file — `{"as_of": <RFC 3339>, "venue": "...", "tolerance": "<decimal>",
"holdings": [{"asset": "...", "quantity": "<decimal>", "tolerance": "<decimal>"}]}`,
decimals as strings and never JSON numbers, each holding's `tolerance` optional
where the statement sets one. The root reads it at start and refuses to start on
a malformed file, a future `as_of`, an empty holdings list or more than 256
holdings, naming the field. A holding that states its own `tolerance` is refused
when that figure is not smaller than the magnitude of the `quantity` beside it —
one transposed pair, caught on the document's own face; the complete bound is
the fabric's, against what the ledger expects, and a holding on the statement's
default tolerance is judged only there. An admitted `POST /cycle` re-reads the
file when its modification time or length has moved, and refuses the cycle with
`503` naming the variable if the file has gone or stopped parsing. Unset means
no feed, the banner says so, and this body answers `assembled: false`. No
deployment mounts a statement yet; `manifest_wiring.rs` records why.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "assembled": true,
  "reason": null,
  "as_of": "2025-10-09T08:53:20.000Z",
  "holdings": [
    {
      "venue": "simulated-venue",
      "asset": "USD",
      "observed_quantity": "10000000",
      "observed_at": "2025-10-09T08:52:20.000Z",
      "provenance": "statement",
      "ledger_expected": "10000000"
    }
  ],
  "reconciliation": {
    "outcomes": [
      { "outcome": "reconciled", "venue": "simulated-venue", "asset": "USD", "delta": "0" }
    ],
    "halted_venue_assets": 0
  }
}
```

| Key | Type | Meaning |
|---|---|---|
| `assembled` | bool | Whether the journal holds a wallet. `false` until a statement has been handed in and a cycle has assembled against it. |
| `reason` | string or `null` | Why not, when `assembled` is `false`. |
| `as_of` | timestamp or `null` | The instant the wallet was assembled at — the cycle's LEARN stage. |
| `holdings[]` | list | One per observed venue-asset, in venue-then-asset order. `provenance` is one of `read_only_api_key`, `watch_only_address`, `view_key`, `statement`; this process only ever records `statement`. `ledger_expected` is `ledger_balance - reserved + in_flight` as a money string, or `null` where the ledger books nothing at that venue-asset. |
| `reconciliation.outcomes[]` | list | The fabric's own record per venue-asset, in the same order: `{"outcome": "reconciled", "venue", "asset", "delta"}` or `{"outcome": "halt", "venue", "asset", "delta", "alert": {"cause": "delta_beyond_tolerance" \| "unrecorded_by_ledger", "expected", "observed", "delta", "tolerance", "observed_at", "provenance", "message", ...}}`. A halt instructs; nothing auto-corrects. |
| `reconciliation.halted_venue_assets` | integer | Count of `outcomes` whose `outcome` is `"halt"`. |

## `GET /api/v1/corridors`

The corridor registry and the destination allowlist the kernel's fabric
journal holds, as records with lifecycle stage and caps. Both are held from
assembly — an allowlist that permits nothing is the safe default, not an
absence — and every record is one a command through the journal proposed.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "corridors": {
    "held": true,
    "reason": null,
    "records": [
      {
        "id": "treasury-sweep",
        "source": { "region": "home", "currency": "USD", "venue": "simulated-venue" },
        "source_class": "fiat_at_institution_of_record",
        "kind": "institution_approval_flow",
        "destination": { "asset": "USD", "address": "treasury-account" },
        "caps": {
          "max_per_transfer": "1000", "max_per_hour": "1000", "max_per_day": "5000",
          "max_cumulative": "10000", "min_interval_seconds": 3600,
          "permitted_hours": { "start": 0, "end": 24 }
        },
        "purpose": "sweep realised cash to the treasury account",
        "stage": "proposed",
        "proposed_by": "treasury-desk",
        "proposed_at": "2025-10-09T08:53:20.000Z",
        "reviewed_by": null,
        "reviewed_at": null,
        "signed": false,
        "activation_at": null
      }
    ]
  },
  "destinations": {
    "held": true,
    "reason": null,
    "records": [
      { "asset": "USD", "address": "treasury-account", "status": "proposed", "proposed_by": "treasury-desk", "proposed_at": "2025-10-09T08:53:20.000Z", "usable_from": null }
    ]
  }
}
```

| Key | Type | Meaning |
|---|---|---|
| `corridors.held` / `destinations.held` | bool | Whether the process holds the registry. `true` from assembly. |
| `corridors.reason` / `destinations.reason` | `null` | Kept for the contract; set only if a registry were ever not held. |
| `corridors.records[]` | list | In corridor-id order. `stage` is one of `proposed`, `reviewed`, `signed`, `time_delayed`, `active`, `suspended`, `revoked`; `source_class` and `kind` are the fabric's custody-table labels. Money as strings. |
| `destinations.records[]` | list | In key order. `status` is one of `proposed`, `verified`, `signed`, `revoked`; `usable_from` is set only while `signed`. |

## `GET /api/v1/transfer-gate`

The seven deterministic checks of blueprint §37.3 in assessment order, the
newest assessment the fabric journal holds, and the platform's kill switch,
which is the state the gate's seventh check reads. An intent reaches the gate
only as a command through the kernel's fabric journal, so every assessment is
a record in the event log; nothing in this process consumes an approval.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "checks": [
    { "order": 1, "name": "corridor_authority", "alerts": true },
    { "order": 2, "name": "caps", "alerts": false },
    { "order": 3, "name": "minimum_interval", "alerts": false },
    { "order": 4, "name": "stated_purpose", "alerts": false },
    { "order": 5, "name": "source_balance", "alerts": false },
    { "order": 6, "name": "velocity_and_anomaly", "alerts": true },
    { "order": 7, "name": "kill_switch", "alerts": false }
  ],
  "last_assessment": null,
  "kill_switch": {
    "halted": false,
    "halted_scopes": [],
    "tripped_by": null,
    "reason": null,
    "tripped_at": null
  },
  "executes": false,
  "note": "the gate is veto-only and has no transfer engine behind it: ..."
}
```

| Key | Type | Meaning |
|---|---|---|
| `checks[]` | list of 7 | From the fabric's own `GateCheck::ALL`, in the order the gate runs them. `alerts` is whether §37.3 pairs a veto by that check with an alert to a person. |
| `last_assessment` | object or `null` | The newest assessment in the journal: `{corridor, assessed_at, outcome: "approved" \| "vetoed", check, reason, alert}`. `check` and `reason` are set for a veto and `null` for an approval. `null` while no intent has been assessed. |
| `kill_switch.halted` | bool | The platform's global kill switch — the same fact `/risk` and `/system` serve. |
| `kill_switch.halted_scopes` | string[] | Strategies or instruments halted individually. |
| `kill_switch.tripped_by` / `reason` / `tripped_at` | string or `null` | Set only while halted. |
| `executes` | `false` | Constant. Names the property the frontend must render: this gate cannot move anything. |
| `note` | string | Prose for the page. |

## Errors

Same as the rest of the API: `401` with `www-authenticate: Bearer` for a
missing or unknown token, `403` below the route's role (`analyst` for
`/ledger/users`, `viewer` for the other three), `429` over the rate limit,
`405` for a method other than `GET`, `500` naming the reason if a view refuses
to build (a ledger expectation that overflows, a verdict in a shape the reader
does not know), `503` if the platform lock is poisoned.
