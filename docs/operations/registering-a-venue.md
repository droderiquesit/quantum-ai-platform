# Registering with a venue

The platform refuses to read a source that needs an account until a named
person has registered for it. This is what to do, in order, and what the
platform refuses until each step is done.

## Why this is a runbook and not a service

The request that produced this page was for a scraper that self-registers
for exchange and venue APIs, anonymously. That is refused, and not as a
matter of taste:

- Automated signup circumvents the venue's terms and its identity checks,
  which are the venue's way of knowing who is bound by what. A licence
  nobody read is one nobody can be held to, and an account nobody owns is one
  nobody can be asked about.
- This platform's rules put licensing posture *before* use
  (`.claude/rules/domains/data-and-streaming.md`) and put reading a vendor's
  terms on the owner (ADR 0034, ADR 0040 — "a record cannot read them on the
  owner's behalf").

The refusal is in the code, not only here. `RegistrationRecord` in
`backend/crates/services/qip-data-finder/src/registration.rs` has one
constructor and it refuses a blank operator; the deserialiser goes through
the same constructor. Both admission gates — `admission::admit*` for the
connector feed and `DataFinder::assess` for discovered sources — refuse a
source whose requirement is not `keyless` until a record exists, and the
refusal carries this sentence verbatim:

> anonymous or automated registration is not a path this platform offers

## What each source needs

Declared in `RegistrationRegistry::shipped`, from the connector manifests
under `backend/crates/services/qip-market-ingestion/src/connectors/manifests/`
and ADR 0034. The requirement is a floor: raise it when the terms say more.

| Source | Requirement | Basis |
|---|---|---|
| `coinbase-spot-ticker` | `keyless` | Manifest `auth: none`; "free, unauthenticated, no signup" |
| `frankfurter-ecb-reference-rates` | `keyless` | Manifest `auth: none`; "free, unauthenticated, no signup" |
| `alpaca-daily-bars` | `account` | Manifest reads `QIP_ALPACA_API_KEY_ID` and `QIP_ALPACA_API_SECRET_KEY`; "an account is required". Whether opening it involves identity verification is not stated anywhere in this repository — when you read the terms, raise this to `account_with_identity_verification` if it does |
| `kalshi-markets` | `account` | Manifest reads an unauthenticated endpoint, but Kalshi is a CFTC-regulated market whose API terms are unread (ADR 0034); whether an anonymous reader is permitted is what the terms would say, so the default is the restrictive one until you read them |

The two `account` rows are also refused by the licensing gate for a separate
reason — their terms are unread — and that refusal comes first. Read the
terms, write the catalogue entry in `admission.rs`, and only then does the
registration question arise.

## The steps

1. **Read the terms.** The venue's API terms, market-data terms and account
   agreement, against the two usages the gate asks about — `derive` and
   `trade` (paper today; ADR 0023). Note the URL or document name and the
   instant you read it: both go in the record. If the terms forbid either
   usage, stop; the catalogue entry records that and the source stays
   refused.
2. **Register under your own identity.** Your name, your organisation, your
   e-mail. If the venue asks for identity verification, that is you doing it,
   with your documents. Nothing in this repository will do this step, and a
   request to automate it is refused.
3. **Create the API key in the venue's dashboard.** Give it the narrowest
   scope the venue offers — read-only market data where that exists. Never
   a trading scope: this platform never submits a live order (ADR 0003), and
   a key that could is a key with a capability nobody here can use.
4. **Put it in Secret Manager as a `_FILE`-projected secret.** The value goes
   into the Secret Manager container the environment already declares (ADR
   0040 §"what the authorisation cannot reach"), with
   `gcloud secrets versions add`, run by you. The deployment projects it as a
   file and the process reads it through `qip_core::secret`, which resolves
   `<VARIABLE>_FILE` — for Alpaca, `QIP_ALPACA_API_SECRET_KEY_FILE` and
   `QIP_ALPACA_API_KEY_ID_FILE`. The value never goes in an environment
   variable, a manifest, a config file, a commit, or a chat.
5. **Record the registration in the platform's config.** A
   `RegistrationRecord` with:
   - `source_id` — the manifest's `source_id`;
   - `operator` — you, in a form the venue and the audit trail both
     recognise;
   - `terms_read_at` — the instant from step 1;
   - `terms` — the URL or document name from step 1;
   - `secret` — the *variable name* the manifest reads the credential from
     (`QIP_ALPACA_API_SECRET_KEY`), never the value. The type refuses
     anything that is not a `SCREAMING_SNAKE_CASE` variable name, so a pasted
     key is refused at load without being echoed.

   In JSON, the shape a config file carries:

   ```json
   {
     "source_id": "alpaca-daily-bars",
     "operator": "<your name, as the venue knows it>",
     "terms_read_at": "<RFC 3339 instant>",
     "terms": "https://alpaca.markets/terms-and-conditions",
     "secret": { "variable": "QIP_ALPACA_API_SECRET_KEY" }
   }
   ```

   The composition roots do not yet read this file; today a root passes the
   registry to `admission::admit_registered`. Until a root does, the
   shipped registry records nobody and the `account` sources stay refused —
   which is the correct state for a registration nobody has made.
6. **Check what the platform now says.** The admission banner names the
   licence, the usages, and `registered by <operator> under <terms> read at
   <instant>, credential named <variable>`. If it says `keyless` for a source
   you registered, the requirement is declared wrong; if it refuses, the
   refusal names the step you missed.

## What the platform refuses until then

- A source with no declared requirement: refused as unknown. Unknown is not
  keyless; declare `keyless` explicitly for a public endpoint.
- A source needing an account with no record: refused by name, with who must
  register and the sentence above.
- A record with a blank operator: cannot be constructed, from code or from a
  file.
- A record whose secret reference looks like a key: refused, and the value is
  not repeated in the refusal.
- A record for a source whose requirement was never declared: refused, so a
  later `keyless` declaration cannot erase the fact that someone had to
  register.

## What may be automated, and what may not

**May be automated, under your own logged-in session:** navigating a
dashboard you are already signed into. A Playwright script that opens the
venue's API-keys page, reads the key you just created and writes it straight
into `gcloud secrets versions add` — never to disk, never to a log — is
saving you a copy-and-paste, and the session, the account and the terms are
yours. The script runs on your machine under your session cookie; it does not
run in this platform, and no credential it handles ever reaches a
repository file.

**May not be automated, by anyone, for anyone:**

- Signup. The account is a person's agreement with the venue.
- CAPTCHAs and bot checks. They exist to tell that a person is present; a
  script that defeats one is a statement that none was.
- Identity verification. Your documents, your face, your consent.
- Accepting terms. Reading them is the whole point of step 1.
- Creating accounts under names, e-mails or identities that are not yours,
  or rotating through them to evade a rate limit or a ban.

If a step in a venue's flow cannot be completed without one of those, that
step is yours, and the platform waits.

## The signup job

`scripts/venue-signup/signup.mjs` types a venue's signup form under the
company's own identity, after a named operator has approved that venue's
terms, and stops at every step the list above says is a person's. It is
dev tooling for the operator's machine; nothing in `backend/` or `frontend/`
imports it. It does not change what may be automated: the approval record
is the person's agreement, and the job is the typing.

```
COMPANY_IDENTITY_FILE=/run/company/identity.json \
node scripts/venue-signup/signup.mjs --venue alpaca --approval approval.json
node --test scripts/venue-signup/signup.test.mjs
```

**Inputs.** `--venue <id>` names a recipe at
`scripts/venue-signup/recipes/<id>.json` — committed, reviewed data listing
the signup URL and the form as `{selector, field}` pairs, with no secret in
it; a key a recipe may not carry is refused, and so is a plaintext URL
anywhere but loopback. `COMPANY_IDENTITY_FILE` is a JSON file outside the
repository holding exactly `legal_name`, `contact_email`, `phone`, `address`
and `country` (ISO 3166-1 alpha-2); a sixth field is refused by name, and a
value shaped like a credential or a tax id (the model gateway's screen plus
SSN, EIN, NINO and nine-digit shapes) is refused without being echoed.
`--approval <path>` is the platform's registration record exported as JSON
plus the Secret Manager slot names: `source_id` (must match the recipe),
`operator` (blank is the anonymous registration this page refuses),
`terms_read_at` (refused if absent, unparseable, in the future, or older
than 24 hours), `terms` (must equal the reference the recipe's terms
checkbox cites, or the box is not ticked and the run stops), and
`secret_slots.password` with `secret_slots.api_key` when the recipe reads a
key on success.

**What it does.** Refuses before a browser exists if `gcloud` is not on
PATH, if `NODE_TLS_REJECT_UNAUTHORIZED=0` is set, or if the recipe declares
`identity_verification_required: true` (`kalshi.json` does, and the job says
the account must be opened by a person). Otherwise it opens the page,
inventories every visible field, and only if every field is one the recipe
lists fills them from the identity file with a 32-character password from
the CSPRNG held only in memory, ticks the one terms box the approval names,
and submits. On success the password, and any API key the page shows, go to
`gcloud secrets versions add <slot> --data-file=-` on stdin — never to disk,
never to stdout, never to an argument. Its stdout is one JSON line naming
the outcome and the slots written; the values appear nowhere.

**Hard stops.** A captcha or bot challenge of any kind; an identity-document
upload, selfie, tax-id, SSN or date-of-birth field; a verification or
second-factor code prompt; a consent box the approval does not cover; any
field the recipe does not list. Each is exit 70 with a named reason and a
screenshot under `VENUE_SIGNUP_SCRATCH_DIR` (default
`$TMPDIR/venue-signup/`). None is solved, worked around or retried. Treat
the screenshot as identity data: it shows what was typed, with password
fields masked by the browser. If the stop comes after submit — `alpaca.json`
declares e-mail verification as exactly that — the password is written to
its slot first, because the venue may have created the account and losing
the password would leave one nobody can enter; the verification is yours.
Exit codes: 64 usage, 65 recipe, 66 identity, 67 approval, 69 prerequisite,
70 hand-back, 71 the venue did something the recipe does not describe, 72 a
secret could not be stored (the reason says which, and what to recover).

**What it cannot do, and why.** It cannot read your mail, so an e-mail
verification ends every run that reaches one, and for Alpaca that is every
run: the account exists, the password is in its slot, and the code is
yours. It cannot open a Kalshi account at all, because that means the
venue's identity verification — your documents, your face — and it refuses
before a browser opens. It cannot tell a venue a person is present, so a
captcha is a hand-back even when it is a trivially "solvable" one; a script
that defeats one is a statement that nobody was there. It cannot create the
API key Alpaca issues from the dashboard after signup — that is step 3, with
the read-only scope, by you. It has never been run against a real venue:
the two recipes are reviewed data whose selectors were written from the
public forms and not exercised against them, and the first real run should
be watched, knowing that a field the page has and the recipe does not is a
hand-back rather than a guess. It drives Chromium over the DevTools
protocol with plain Node rather than Playwright, because Playwright is not
resolvable from this repository and a browser-automation dependency for one
job is not worth the supply chain; the browser is the one Playwright's
bundle provides at `/opt/pw-browsers/chromium` (override with
`VENUE_SIGNUP_CHROMIUM`), launched with `--remote-debugging-pipe` so no port
is opened for anything else to attach to. It honours `HTTPS_PROXY` and
`NO_PROXY` as the rest of the repository does, and if the egress proxy's
certificate is not in the system store it fails on TLS — the fix is the CA,
never `--ignore-certificate-errors`, which it does not pass. Run as root it
drops Chromium's own sandbox (`--no-sandbox`) and says so; do not run it as
root. Nothing here reads the registration record the platform holds or
writes one: after a successful run, step 5 is still yours.
