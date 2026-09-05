# Ledger, wallet, corridor and transfer-gate routes

The read-only treasury surface of `qip-api`. Four `GET` routes under
`/api/v1`, all at the `viewer` role, all answering `200` with
`content-type: application/json`. `POST`, `PUT`, `PATCH` and `DELETE` on any
of them answer `405 {"error":"that method is not allowed here"}`. Nothing here
submits, approves, signs or moves anything, and there is no route that could:
ADR 0021 permits the deterministic half of the blueprint's treasury and refuses
the path by which capital leaves the platform.

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
  id, then strategy id, then currency code) so two reads of the same state
  render identically.
- Absence is stated, never zero-filled: where the platform does not hold a
  subsystem the body says so with a boolean and a `reason` string.

## `GET /api/v1/ledger/users`

Every user enrolled in the per-user ledger with their mandate, their
per-strategy balances and the entitlement evaluation for the viewer role.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "evaluated_as_role": "viewer",
  "products": ["research-tests"],
  "fills_journalled": 0,
  "users": [
    {
      "user_id": "desk",
      "mandate": {
        "capital": "1000000",
        "currency": "USD",
        "risk_tolerance": "1",
        "liquidity_floor": "0",
        "investable": "1000000",
        "exploration_share": "0",
        "jurisdiction": "ZZ",
        "permitted_families": { "any": true, "families": [] }
      },
      "balances": [
        {
          "strategy": "AAA",
          "currency": "USD",
          "settled": "250.75",
          "reserved": "0",
          "available": "250.75",
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
          "can_view": { "granted": true, "reason": "desk holds a mandate in ZZ" },
          "can_invest": { "granted": false, "reason": "desk holds the viewer role, which does not invest" },
          "can_withdraw": { "granted": false, "reason": "capital does not leave the platform: ADR 0021 refuses the signing and withdrawal half of the treasury and ADR 0023 keeps that in force; a withdrawal is a separate, later, separately approved decision" }
        }
      ],
      "entitlements_note": null
    }
  ]
}
```

Field by field:

| Key | Type | Meaning |
|---|---|---|
| `evaluated_as_role` | `"viewer"` | The ledger role every entitlement was evaluated under. This surface is the viewer's; it never evaluates as an investor or the desk. |
| `products` | `string[]` | The strategy families registered with the central factory, which are the products an entitlement is evaluated against. Empty on a fresh platform. |
| `fills_journalled` | integer | Attributed fills the ledger has booked since assembly. |
| `users[].user_id` | string | The ledger's user id. `"desk"` is the platform's own book. |
| `users[].mandate.capital` | money string | Capital under management. |
| `users[].mandate.currency` | string | ISO 4217 code. |
| `users[].mandate.risk_tolerance` | decimal string in `[0, 1]` | Share of capital the user tolerates losing. |
| `users[].mandate.liquidity_floor` | money string | Capital that stays liquid however strategies are sized. |
| `users[].mandate.investable` | money string | `capital - liquidity_floor`. |
| `users[].mandate.exploration_share` | decimal string in `[0, 1]` | Share spendable on information gain. |
| `users[].mandate.jurisdiction` | 2-letter string | ISO 3166 alpha-2; `"ZZ"` is the desk's own. |
| `users[].mandate.permitted_families` | `{any: bool, families: string[]}` | `any: true` means every family; otherwise `families` lists the only ones. |
| `users[].balances[]` | list | One row per `(strategy, currency)` book the user holds. Empty until a fill has been attributed to the user. |
| `balances[].settled` | money string | Cash the ledger has said is here. |
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

The wallet read model and its reconciliation outcomes. The kernel in this
deployment holds no wallet, so the body reports that and fabricates no holding.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "assembled": false,
  "reason": "no wallet is assembled in this process. A wallet is a read model over holdings observed through read-only channels, and the kernel observes no custodian, venue balance or chain address; until an observation source is wired in there is nothing to pair with the ledger, and a wallet showing zero would read as an empty account rather than an unobserved one.",
  "as_of": null,
  "holdings": [],
  "reconciliation": {
    "outcomes": [],
    "halted_venue_assets": 0
  }
}
```

| Key | Type | Meaning |
|---|---|---|
| `assembled` | bool | Whether a wallet exists in this process. `false` in every current deployment. |
| `reason` | string or `null` | Why not, when `assembled` is `false`. |
| `as_of` | timestamp or `null` | The instant the wallet was assembled at. |
| `holdings[]` | list | Empty until `assembled` is `true`. When populated: `{venue, asset, observed_quantity, observed_at, provenance, ledger_expected}` with money as strings and `provenance` one of `read_only_api_key`, `watch_only_address`, `view_key`, `statement`. |
| `reconciliation.outcomes[]` | list | Empty until `assembled` is `true`. When populated, each is the fabric's own record: `{"outcome": "reconciled", "venue", "asset", "delta"}` or `{"outcome": "halt", "venue", "asset", "delta", "alert": {...}}`. A halt instructs; nothing auto-corrects. |
| `reconciliation.halted_venue_assets` | integer | Count of `outcomes` whose `outcome` is `"halt"`. |

## `GET /api/v1/corridors`

The corridor registry and the destination allowlist, as records with lifecycle
stage and caps. The kernel in this deployment holds neither registry.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "corridors": {
    "held": false,
    "reason": "no corridor registry is held in this process. A corridor is the signed record of where capital may go and under what caps; the kernel composes no treasury and has proposed, reviewed or signed none, so there is no corridor to list — not an empty registry that admits nothing, but no registry at all.",
    "records": []
  },
  "destinations": {
    "held": false,
    "reason": "no destination allowlist is held in this process. ...",
    "records": []
  }
}
```

| Key | Type | Meaning |
|---|---|---|
| `corridors.held` / `destinations.held` | bool | Whether the process holds the registry. `false` in every current deployment. |
| `corridors.records[]` | list | Empty until held. Record shape: `{id, source: {region, currency, venue}, source_class, kind, destination: {asset, address}, caps: {max_per_transfer, max_per_hour, max_per_day, max_cumulative, min_interval_seconds, permitted_hours: {start, end}}, purpose, stage, proposed_by, proposed_at, reviewed_by, reviewed_at, signed, activation_at}`. Money as strings; `stage` one of `proposed`, `reviewed`, `signed`, `time_delayed`, `active`, `suspended`, `revoked`. |
| `destinations.records[]` | list | Empty until held. Record shape: `{asset, address, status, proposed_by, proposed_at, usable_from}` with `status` one of `proposed`, `verified`, `signed`, `revoked`. |

## `GET /api/v1/transfer-gate`

The seven deterministic checks of blueprint §37.3 in assessment order, the
last assessment (there has never been one — the gate has no caller in this
process), and the platform's kill switch, which is the state the gate's
seventh check reads.

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
  "note": "the gate is veto-only and has no transfer engine behind it: an approval is a record that the seven checks passed, and nothing in this platform consumes one. No caller has yet assessed an intent."
}
```

| Key | Type | Meaning |
|---|---|---|
| `checks[]` | list of 7 | From the fabric's own `GateCheck::ALL`, in the order the gate runs them. `alerts` is whether §37.3 pairs a veto by that check with an alert to a person. |
| `last_assessment` | `null` | Always `null` in this deployment. Were an assessment ever recorded it would be `{assessed_at, outcome: "approved" \| "vetoed", check, reason, alert}`. |
| `kill_switch.halted` | bool | The platform's global kill switch — the same fact `/risk` and `/system` serve. |
| `kill_switch.halted_scopes` | string[] | Strategies or instruments halted individually. |
| `kill_switch.tripped_by` / `reason` / `tripped_at` | string or `null` | Set only while halted. |
| `executes` | `false` | Constant. Names the property the frontend must render: this gate cannot move anything. |
| `note` | string | Prose for the page. |

## Errors

Same as the rest of the API: `401` with `www-authenticate: Bearer` for a
missing or unknown token, `403` below the viewer role, `429` over the rate
limit, `405` for a method other than `GET`, `503` if the platform lock is
poisoned.
