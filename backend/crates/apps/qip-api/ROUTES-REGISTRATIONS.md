# Venue registration routes

The venue-registration surface of `qip-api`: one `GET` and one `POST` under
`/api/v1`, both answering `content-type: application/json`.
`GET /registrations` requires the `viewer` role; `POST
/registrations/{source}/approve` requires `operator`. `PUT`, `PATCH` and
`DELETE` on either answer `405 {"error":"that method is not allowed here"}`.

What this surface is for. The platform refuses to read a source that needs
an account until a named person has registered for it
(`backend/crates/services/qip-data-finder/src/registration.rs`;
`docs/operations/registering-a-venue.md`). The platform does everything
about that registration except the part that must be a person's: the list
says, per catalogued source, what the venue demands, where the source
stands, which terms to read, which deployment variable the credential is
read under and the one command that fills it; the approval records that the
authenticated operator did those things. Nothing here signs up for anything,
reads a venue's terms on anyone's behalf, or carries a credential's value in
either direction. The refusal for a source nobody has registered carries
this sentence verbatim:

> anonymous or automated registration is not a path this platform offers

Every body is read off the kernel at request time. Standing is the
registration registry's own answer through `Platform::registration_standing`
— the same registry the feed's admission gate consults when a connector is
opened at start, so the page and the gate cannot disagree about who
registered. An approval goes through `Platform::approve_registration`, which
journals the record to the event log before the registry adopts it;
`Platform::replay_registrations` rebuilds the registry from the log alone.
The API keeps no copy of any of it.

The Rust shapes are in `src/registration_views.rs`. This file is the same
contract in prose, kept exact so a page can be built against it without
reading Rust.

## Conventions

- Every body carries `"posture": "PAPER TRADING"` as its first key. Render it.
- Every body carries `"served_at"`, the instant the platform answered, in
  RFC 3339 UTC.
- Every timestamp is RFC 3339 UTC or `null`.
- Keys are stable `snake_case`. `sources` is in the finder's catalogue order,
  so two reads of the same state render identically.
- Absence is stated, never zero-filled: a source with no credential slot
  says `null`, not `""`; a source nobody registered says `pending` with the
  reason, not an empty operator.
- **No body carries a credential value, and no request body is accepted
  that could be one.** Every `secret` field is the *name* of a deployment
  variable (`QIP_ALPACA_API_SECRET_KEY`), never what it resolves to. See the
  approval route for what is refused.

## `GET /api/v1/registrations`

**Role: `viewer`.** Every source in the data finder's licensing catalogue,
with what it demands and where it stands.

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "sources": [
    {
      "source_id": "kalshi-markets",
      "requirement": "account",
      "standing": {
        "standing": "pending",
        "who_must_register": "qip-platform",
        "reason": "`kalshi-markets` requires an account with the venue, opened in the operator's own name (requirement `account`) and no registration record exists for it, so it is refused. The platform's owner must register with the venue under their own identity, read its terms, create the credential in the venue's dashboard, place it in Secret Manager as a `_FILE`-projected secret, and record the registration — see docs/operations/registering-a-venue.md. anonymous or automated registration is not a path this platform offers: it circumvents the venue's terms and identity checks, and a licence nobody read is one nobody can be held to"
      },
      "terms": "https://kalshi.com/terms",
      "secret_slot": null,
      "secret_command": null,
      "companion_secret_slots": []
    },
    {
      "source_id": "alpaca-daily-bars",
      "requirement": "account",
      "standing": {
        "standing": "registered",
        "operator": "operator@env",
        "terms_read_at": "2025-10-09T08:50:00.000Z",
        "secret": "QIP_ALPACA_API_SECRET_KEY"
      },
      "terms": "https://alpaca.markets/terms-and-conditions",
      "secret_slot": "QIP_ALPACA_API_SECRET_KEY",
      "secret_command": "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-",
      "companion_secret_slots": [
        {
          "variable": "QIP_ALPACA_API_KEY_ID",
          "secret_command": "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-"
        }
      ]
    },
    {
      "source_id": "coinbase-spot-ticker",
      "requirement": "keyless",
      "standing": { "standing": "keyless" },
      "terms": "coinbase-exchange-market-data-terms",
      "secret_slot": null,
      "secret_command": null,
      "companion_secret_slots": []
    }
  ]
}
```

Per source:

| Key | Type | Meaning |
|---|---|---|
| `source_id` | string | The manifest's `source_id`; the path segment the approval route takes. |
| `requirement` | string or `null` | What the venue demands, from the shipped table: `keyless`, `self_service_api_key`, `account`, `account_with_identity_verification`. `null` when no requirement is declared — the standing is then `pending`, because an unasked question is not a keyless source. |
| `standing` | object | One of the three shapes below. |
| `terms` | string or `null` | The terms reference the catalogue carries: the licence identifier of an evaluated posture, or the URL an unevaluated posture's evidence names. What an operator reads before approving; the record then cites what they actually read. |
| `secret_slot` | string or `null` | The deployment variable the connector manifest reads the credential under — the value an approval's `secret` must carry. `null` for a manifest that names no credential. |
| `secret_command` | string or `null` | The one line an operator runs to put a version behind `secret_slot`. Names the secret only; the value comes from stdin (`--data-file=-`) so it is in no shell history or process listing. |
| `companion_secret_slots` | list | Every further variable the manifest reads (Alpaca's key id beside its secret key), each `{ "variable", "secret_command" }`. Empty for a single-secret or keyless source. |

`standing` is tagged by its own `standing` key:

- `{ "standing": "keyless" }` — no registration needed.
- `{ "standing": "registered", "operator", "terms_read_at", "secret" }` — a
  registration exists. `operator` is the **authenticated subject of the
  credential the approval arrived on** (or the operator named in the
  deployment's committed configuration) — for an approval made through the
  console that is the console's deployment credential, `operator@env`, and
  not the person who clicked; see the 2026-09-05 amendment to ADR 0041.
  `terms_read_at` is the instant the platform answered; `secret` the
  deployment variable the credential is read under. Never the value.
- `{ "standing": "pending", "who_must_register", "reason" }` — refused
  until somebody registers. `who_must_register` is the deployment's
  configured owner; `reason` is the registry's own refusal, verbatim, the
  same sentence the feed's admission gate prints.

The Secret Manager secret in `secret_command` is named by the convention
the environment already uses for every secret it declares
(`QIP_TOKEN_VIEWER` is `qip-token-viewer`, `QIP_VENUE_CREDENTIAL` is
`qip-venue-credential`): the variable in lower case with `_` as `-`. The
environment module suffixes each secret with the environment name at
creation; the command names the secret as the runbooks do, without it. The
secret behind a connector's slot is not yet declared in
`infrastructure/terraform/main.tf`'s `secret_names`; declaring it is an
infrastructure change the command does not perform.

## `POST /api/v1/registrations/{source}/approve`

**Role: `operator`.** Record that the authenticated operator registered
with the venue behind `{source}`, read the terms the body cites, created the
credential and put it in Secret Manager under the variable the body names.

Request body — both fields required, both refused blank:

```json
{
  "terms": "https://alpaca.markets/terms-and-conditions",
  "secret": "QIP_ALPACA_API_SECRET_KEY"
}
```

| Key | Meaning |
|---|---|
| `terms` | The URL or document name of the terms the operator read. Blank is refused: "the terms" without a citation is a claim nobody can re-read. |
| `secret` | The deployment variable the manifest reads the credential under — the list's `secret_slot`. Screened by the manifest's own shape rule (`SecretRef`): a name starts with `A-Z` and continues in `A-Z`, `0-9` and `_`, so a pasted key cannot be written here. |

The operator on the record is the **authenticated principal's subject** —
that is, the subject of the bearer credential this request presented. The
body names nobody and cannot: the route builds an `OperatorIdentity` from the
authenticated principal exactly as `DELETE /kill-switch` does, and the kernel
takes the record's operator from that identity. `terms_read_at` is the
instant the platform answered.

**What that subject is, and is not.** Credentials are configured per *role*,
not per person (`main.rs` builds each one with the subject
`<role>@env`), and the console holds exactly one of them for every browser
session: its gateway attaches one deployment bearer token and forwards no
end-user identity, because the sealed console session is signed with a key
this API does not hold and cannot verify. So an approval made through the
console records `operator@env` — which deployment acted, not which person.
Do not read the `operator` field as a personal attribution. The gap, and the
design that would close it, are the 2026-09-05 amendment to ADR 0041.

Success — `200`, the source's standing after the record was journalled and
adopted:

```json
{
  "posture": "PAPER TRADING",
  "served_at": "2025-10-09T08:53:20.000Z",
  "source_id": "alpaca-daily-bars",
  "standing": {
    "standing": "registered",
    "operator": "operator@env",
    "terms_read_at": "2025-10-09T08:53:20.000Z",
    "secret": "QIP_ALPACA_API_SECRET_KEY"
  }
}
```

Refusals — `{"error": "<reason>"}`:

| Status | When |
|---|---|
| `400` | The body is not a JSON object with `terms` and `secret`; either is missing, not a string, or blank; the body carries a key this route does not read; `secret` is not a deployment variable name; or `{source}` has no registration requirement declared. |
| `403` | The credential holds less than `operator`. Nothing moves. |
| `409` | The operator credential is older than the kernel accepts for an approval (fifteen minutes, the same as an eligibility decision). Re-authenticate. |

**A refusal never repeats the `secret` it refused.** The one case this rule
exists for is a pasted key, and an error that quoted it would write the key
to the response, stderr and whichever ticket the line is copied into. The
shape screen describes the length and the class of the first offending
character (`the secret reference (20 characters) has a lowercase letter at
position 1, so it is not a deployment variable name …`) and nothing else,
and a refused body reaches neither the registry nor the event log.

An approval does not re-open a connector. The feed's admission gate runs
once, at start, against the registry the configuration stands for; a source
approved at runtime stands registered in the platform and in the log, and is
admitted onto the feed at the next start.

## What no route here does

- Registers with a venue, accepts its terms, passes its identity checks or
  creates a key. Those are the operator's, and a request to automate them is
  refused (`docs/operations/registering-a-venue.md`, "What may not be
  automated").
- Accepts, stores, echoes or resolves a credential value. The only `secret`
  in either direction is a variable name.
- Attributes a registration to anyone but the authenticated operator.
- Declares or changes a requirement. The requirement table is a source-file
  literal in `RegistrationRegistry::shipped`, reviewed like code.
