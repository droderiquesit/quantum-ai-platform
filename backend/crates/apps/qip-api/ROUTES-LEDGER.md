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
| `balances[].available` | money string | `settled - reserved`. Expected inflows are **not** in this figure. |
| `balances[].expected_inflows_total` | money string | Sum of declared, unposted inflows. Reported so it is visible; never added to anything. |
| `balances[].expected_inflows[]` | `{reference, amount, declared_at}` | Each declared inflow by the reference the user supplied. |
| `balances[].entries` | integer | Attributed fills booked here. Distinguishes "none booked" from "balance happens to be zero". |
| `balances[].last_entry_at` | timestamp or `null` | |
| `users[].entitlements[]` | list | One evaluation per product in `products`. Empty when `products` is empty. |
| `entitlements[].can_view` / `can_invest` / `can_withdraw` | `{granted: bool, reason: string}` | `reason` is the basis of a grant or the input that refused. `can_withdraw.granted` is **always `false`**; the platform's type has no granted arm. |
| `users[].entitlements_note` | string or `null` | Set when `entitlements` is empty, saying why (no product registered). |

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
holdings, naming the field; an admitted `POST /cycle` re-reads the file when its
modification time or length has moved, and refuses the cycle with `503` naming
the variable if the file has gone or stopped parsing. Unset means no feed, the
banner says so, and this body answers `assembled: false`. No deployment mounts a
statement yet; `manifest_wiring.rs` records why.

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
