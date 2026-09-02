# 0025 — The Rust frontend boundary, and whether Leptos is a dependency this platform takes

**Status:** proposed — the owner decides. This record decides nothing; it
frames a decision two other records have named and left open, and it carries a
recommendation marked as one.
**Would amend, if accepted in one direction:** ADR 0001 (the browser has no
JavaScript), ADR 0012 (a web framework fails the three-part test), and the
"no async runtime" line in `.claude/rules/architecture/00-boundaries.md`.
**Does not touch:** ADR 0003, ADR 0021, ADR 0023. No option below moves the
paper-trading boundary, and a frontend decision is not an occasion to revisit
it.

## Context, as verified in the tree at `851c0ed`

### What the architecture of record asks for

ADR 0022 made the Algorik blueprint the architecture of record and, in item 4,
named Leptos as the target experience layer with `frontend/portal` and
`frontend/landing` transitional (`docs/adr/0022:46-49`). The blueprint says,
in the places that bind:

- §2.1, the technology rule: every application is Rust, and the scope column
  names the frontend; TypeScript is excluded as a primary language
  (`blueprint.md:139`).
- §40: "One Leptos codebase in Rust, shared types with every backend service,
  and an installable progressive web app rather than a native shell"
  (`blueprint.md:3239`). §40.5's layer table repeats it: landing, portal,
  admin views and the PWA, one Leptos codebase, provider Rust
  (`blueprint.md:3345`).
- §40.4: the Phase 13 exit criterion is a push-reliability drill on the
  installable PWA — delivery above 99.5 percent, p99 under 15 seconds over 30
  days — with a native shell built only if it fails (`blueprint.md:3337`).
- §51, Phase 13: portal, portfolio, wallet, strategy marketplace, explanation,
  installable PWA, push drill; eight weeks; exit when every operational
  question is answerable and the kill switch has been exercised from mobile
  (`blueprint.md:4467`).
- Rule 71: mobile is the Leptos PWA (`blueprint.md:4983`).

The companion diagram is more specific about the mechanism than the prose:
"Frontend leptos · plotters SSR with WASM hydration; server-side SVG. PWA for
mobile", "portal-web (Leptos SSR + WASM), portal-pwa (service worker, Web Push,
WebAuthn)", and a scorecard line reading "0 Lines of JavaScript framework"
(`ref/index_text.txt`, one line; search for those phrases). Note the word
*framework*: the diagram does not claim zero JavaScript, and it cannot,
because a service worker is JavaScript in every framework including Leptos.

### What ADR 0001 decided, and why it matters here

ADR 0001 is titled "Rust for everything, including the web interface". It
decided server-rendered HTML with no JavaScript at all, and it rejected
compiling Rust to WebAssembly by name: "`wasm-bindgen` generates a JavaScript
glue layer, so a WebAssembly interface is a Rust interface with JavaScript in
it" (`docs/adr/0001:12-15`). It named its own reversal condition — an
interface that genuinely needs streaming updates — and said server-sent events
would meet it without a client framework (`docs/adr/0001:41-44`). It also
made the claim the blueprint makes: "One language means one set of types. The
view model is the same struct the API serialises" (`docs/adr/0001:36-37`).

So the tree already contains a Rust first-party surface built on exactly the
argument the blueprint uses, and the blueprint's named technology is the one
ADR 0001 explicitly declined. That is the decision this record frames.

### The Rust surface that exists: `qip-web`

`backend/crates/apps/qip-web` is a library, not a binary. ADR 0010 records
why (`docs/adr/0010:44-68`) and
`infrastructure.rs::qip_web_is_a_library_and_stops_being_exempt_the_moment_it_is_not`
(`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:3115`) fails
the suite if that changes. Its manifest declares one dependency, `serde`
(`qip-web/Cargo.toml:10-11`), and
`api_boundary.rs::the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate`
asserts it has **no in-workspace edge at all** — "which is what makes 'the
browser holds no trading logic' a fact about the build rather than a
discipline" (`api_boundary.rs:295-301`).

It renders two page sets, each a pure function from a view model to HTML
(`qip-web/src/pages.rs:1-6`):

| Set | Paths | Where declared |
|---|---|---|
| Nine investment surfaces | `/`, `/opportunities`, `/theses`, `/portfolio`, `/risk`, `/execution`, `/agents`, `/governance`, `/audit` | `pages.rs:21-31` |
| Nine operator console views | `/console`, `/console/regional`, `/console/trading`, `/console/arbitrage`, `/console/ai`, `/console/quantum`, `/console/data-finder`, `/console/risk`, `/console/operations` | `console/mod.rs:43-65` |

Three properties of it are load-bearing for any replacement:

- **Nothing here can act**, except tripping the kill switch, and clearing one
  is deliberately unrenderable (`console/mod.rs:12-19`).
- **A view never invents a number.** `Panel` carries whether a collection was
  reported, is stale, or was never reported, so "zero exposure" and "no cell
  is reporting" are different markup (`qip-web/src/lib.rs:13-18`).
- **Every page carries the posture banner first**, and a halted paper platform
  still says PAPER TRADING (`pages.rs:152-195`).

`qip-api` links it and serves it on its own port under the `Viewer` role —
"a page is not a lower bar than the JSON it renders" (`qip-api/src/web.rs:3-6`)
— behind `default-src 'none'; style-src 'self'; img-src 'self'; form-action
'self'; frame-ancestors 'none'; base-uri 'none'` (`qip-api/src/http.rs:200-201`).

Its tests: fifteen in `qip-web/tests/web.rs` (escaping, no script element, the
banner on every surface, a halted paper platform still saying paper, an order
row stating whether it was real, surfaces round-tripping through their paths;
`web.rs:19-296`) and nineteen in `qip-web/tests/console.rs` (absent versus
zero, stale versus current, the kill switch trippable and never clearable, no
off-origin reference, a quantum result never shown without its classical run;
`console.rs:304-692`). These are unit tests of rendering; nothing drives a
browser against them.

### The TypeScript surface that exists: the portal and the landing

`frontend/portal` is Next 16.3.2 on React 19.2.8 with `chart.js`
(`frontend/portal/package.json:28-31`), plus eleven workspace packages under
`frontend/packages/` (ADR 0014). It has **fifty-four** `page.tsx` files:
nine under `(auth)`, eleven under `(marketing)`, thirty-four under `(portal)`
(`frontend/portal/src/app/**`; recount with a glob before quoting). The
`(portal)` group is the customer console proper — overview, portfolio and its
P&L and positions, risk and its limits and audit, orders, execution, markets,
intelligence, research, models, agents, capital, data sources, strategies,
signals, operations, command, admin, system, loop, offline.

`frontend/landing` is a separate Next application in JavaScript — twelve
`page.js` files under `frontend/landing/app/`, `jsconfig.json`, its own
lockfile — deliberately not a workspace member (`frontend/CLAUDE.md`).

Its behavioural contract is Playwright: seven spec files, thirty-one `test(`
calls counted with `grep` (`frontend/portal/tests/*.spec.ts`; recount before
quoting). The ones a Rust replacement has to answer to by name:

- `boundary.spec.ts:21` — no surface composes or submits an order;
  `:53` — the console can halt and offers no way to clear; `:75` — a halted
  platform is shown as halted, not quiet.
- `shell.spec.ts:31` — the paper trading declaration is on every route;
  `:49` — it survives an unreachable platform; `:84` — an unreachable platform
  is never rendered as an empty book.
- `auth.spec.ts:30,77,82,121,132,141` — the whole sign-up/verify/sign-in
  journey, a session-less browser refused, a tampered cookie reading nothing,
  CSRF, an off-origin `next` refused, intent preserved across sign-in.
- `mobile.spec.ts:38,138,159` — the kill switch reachable at phone width, an
  installable manifest that says paper trading, an offline page that shows no
  figures.
- `theme.spec.ts:36,48` — the environment badge is a report not a control; a
  simulated page cannot be read without its label.
- `marketing.spec.ts:26,42` — every public page states the paper boundary and
  makes no claim the platform may not make.

The gates are `npm run lint`, `npm run build` and Playwright, run by
`.github/workflows/ci.yml:147-172` for the portal and `:174-186` for the
landing.

### Three facts that change the shape of the question

1. **"Shared types with every backend service" does not exist today on either
   side.** `frontend/packages/shared-types/src/index.ts` contains two string
   unions, `EnvironmentMode` and `TradingPosture` (`index.ts:12,21`), hand
   written and not generated from anything in `backend/`. On the Rust side,
   `qip-web`'s `ViewModel` is assembled in `qip-api/src/web.rs` from kernel
   types — that is the one set of types ADR 0001 claims, and it is real, but
   it is `qip-api`'s and `qip-web`'s, not "every backend service's".

2. **Neither TypeScript application is provisioned or deployed by the tree.**
   `infrastructure/terraform/catalogue.tf` declares three Cloud Run services
   — `qip-api` (`:42`), `qip-fastbrain` (`:116`), `qip-deepbrain` (`:164`) —
   and `.github/workflows/deploy.yml` names neither `portal` nor `landing`
   (grep, no match). What remains of ADR 0018's console path is the egress
   subnet and identity created only when `console_egress_cidr` is set
   (`infrastructure/terraform/main.tf:264`). ADR 0018 describes a Next.js
   server on Cloud Run; at `851c0ed` nothing in the tree creates that
   service. Whether one exists in any project is not answerable from here.
   The consequence for this record: "cut over in a deployment, then retire"
   has no deployment on either side to cut over in, and ADR 0024 already
   records that nothing has been applied anywhere.

3. **Identity lives in the Next.js server, not in `qip-api`.** The sealed
   session cookie, CSRF and the gateway that presents the viewer token to
   `qip-api` are portal server code (ADR 0019, `frontend/packages/auth`,
   `auth.spec.ts`). `qip-api` authenticates by bearer token and role
   (`qip-api/src/main.rs:120-124`). A Rust replacement of the portal has to
   reproduce the browser session layer somewhere Rust, and that is the hard
   part of the migration, not the pages.

### The dependency policy the question lands on

- `scripts/check-dependencies.sh:19-35` permits eleven packages, all serde's
  closure, and holds the whole lockfile to them.
- `architecture.rs::no_crate_declares_a_third_party_dependency_beyond_the_two_permitted`
  (`architecture.rs:1146`) holds every crate, not only the decision core, to
  the two; ADR 0009 says that strictness stays "until the first client lands".
- ADR 0012's three-part test admits a dependency only where getting it wrong
  is silent, the problem is adversarial or specialist, and a mature audited
  implementation exists — and it refuses "a web framework" by name because it
  fails loudly (`docs/adr/0012:60-62`). It refuses an async runtime on the
  same page (`:53-58`).
- ADR 0013 refuses a browser SDK for bringing "a large transitive tree into
  the browser" (`docs/adr/0013:50-55`).
- `.claude/rules/architecture/00-boundaries.md`: "No new async runtime.
  Blocking I/O with explicit timeouts is a decision, not an omission."

What Leptos brings is therefore the crux. Stated as claims about the
ecosystem for the owner to verify against a lockfile before deciding, not as
facts about this tree: Leptos server rendering ships through async server
integrations (Axum or Actix), which bring an async runtime; hydration and
client-side rendering compile through `wasm-bindgen`, `js-sys` and
`web-sys`, which ADR 0001 refused by name; the build uses `cargo-leptos`; and
the transitive closure is in the hundreds of crates. **A Leptos decision is
not an addition under ADR 0009's edge tier. It is a reversal of two settled
refusals — the async runtime and `wasm-bindgen` — and the record that admits
it has to say so and supersede them.**

## The decision to be taken

Three questions, in the order they depend on one another.

**(a) What is the canonical Rust replacement boundary, and which surface moves
first?** The boundary can only be one of: `qip-web` extended, served by
`qip-api`; or a new Rust application crate in `backend/crates/apps/` that
serves the portal and owns its session layer; or a Leptos application that is
both.

**(b) Is a browser-side Rust framework a reversal condition, or does the
server-rendered `qip-web` path already satisfy "Rust first-party"?** Either
answer is coherent; what is not coherent is treating Leptos as a routine
dependency request.

**(c) What evidence closes the cutover?** The owner's brief demands functional
parity and then retirement. This record says what parity would have to mean
against the contracts that exist.

## Options

### Option A — `qip-web`, server-rendered, is the Rust first-party surface; Leptos is not taken

The boundary is the one ADR 0001 drew and ADR 0010 shaped: HTML rendered from
a Rust view model, served by `qip-api`, no JavaScript framework, CSP
`default-src 'none'`. The portal's fifty-four pages are re-expressed as
`qip-web` surfaces; the landing's twelve static pages likewise. The session
layer moves into `qip-api` or a new `apps/` crate in Rust, in-tree, on the
lines ADR 0013 already drew (session issuance and CSRF stay in-tree; token
verification is the one thing that may take a dependency).

*What it satisfies from the blueprint:* §2.1 (Rust, no TypeScript), "0 lines
of JavaScript framework", server-side SVG for charts (ADR 0001 named it), the
CSP posture, one set of types on the API side.

*What it does not satisfy:* the name Leptos; WASM hydration; client-side
interactivity. The PWA is reachable — a manifest is JSON and a service worker
is a small hand-written script in any framework, which is what
`frontend/portal` already does (`worker.spec.ts`) — but "no JavaScript at
all" (ADR 0001) becomes "no JavaScript framework", and that amendment to 0001
must be written down, not slid past.

*What it costs:* every interaction is a round trip (ADR 0001:24-26); a
streaming blotter would need server-sent events, which `qip-api` already
speaks; charts are server-rendered SVG, which the blueprint's §47 asks for
anyway. Rewriting thirty-four console pages plus auth as Rust is weeks of
work with a Playwright-shaped parity harness that does not exist yet.

*No new dependency.* `check-dependencies.sh` stays at eleven.

### Option B — Leptos, SSR with WASM hydration, as the blueprint names it

A new application crate — call it what the diagram calls it, `portal-web` —
in `backend/crates/apps/`, on Leptos with a server integration and hydration,
serving landing, portal and PWA from one codebase over types shared with
`qip-contracts` and the API's response structs.

*What it satisfies:* the blueprint by name and by mechanism; genuine shared
types across the browser boundary; client-side interactivity without a
second language.

*What it costs:* an async runtime in a deployed process, which reopens ADR
0012's refusal and the boundaries rule; `wasm-bindgen`, which reopens ADR
0001's refusal; a transitive tree the platform cannot read, in the process
that renders posture — though not in the decision core, which ADR 0009's
tiering keeps at two; a build tool outside cargo; and `check-dependencies.sh`
moving from an exact list to a per-tier check, the cost ADR 0009 said would
arrive with the first client. The CSP would need `script-src` and
`wasm-unsafe-eval`, so "cross-site scripting is something the policy makes
impossible" (ADR 0001:30-32) becomes a discipline again.

*Dependency direction:* an `apps/` crate depends inward on libs and services
and nothing depends on it, so the layering holds. What changes is not the
graph's shape but the size of what sits at its top.

*Requires:* a superseding ADR for 0001 and 0012's async-runtime refusal, an
amendment to `.claude/rules/architecture/00-boundaries.md` (the owner's, not
an agent's), a widened `PERMITTED` list with a reason per crate, and the tier
test in `architecture.rs` relaxed for one named crate while still holding the
core.

### Option C — `qip-web` now; Leptos deferred to a measured need, and pre-classified as a reversal

Option A as the canonical boundary and the first slices, with this record
also settling in advance what a future Leptos request is: not a dependency
request under ADR 0009's edge tier but a reopening of two refusals, admitted
only against ADR 0001's own reversal condition — an interface that measurably
needs client-side state — and only by an ADR that supersedes 0001 and amends
0012. That keeps the blueprint's direction (Rust, shared types, PWA) and
declines its mechanism until the mechanism has earned its cost.

*What it costs:* it departs from the architecture of record by name, and every
traceability row citing §40 has to say "Rust, server-rendered; Leptos
deferred" rather than ALIGNED. It also leaves the question open, which is a
cost ADR 0022 already pays for the runtime.

### Option D — keep Next.js and amend the blueprint (rejected here, listed for completeness)

Coherent, cheap, and the opposite of what the owner said. ADR 0022 item 4
makes the TypeScript surface transitional; only the owner can unmake that,
and this record does not argue for it. It is listed because the cutover
evidence in (c) is the same work whichever direction is chosen, and because
an owner who reads the cost of A and B and chooses D should do so in an ADR
rather than by letting the migration lapse.

## The first vertical slice, whichever of A, B or C is chosen

The traceability matrix's Layer 1 row and C3 both say: identify contracts and
Playwright coverage first, then a vertical slice, only if it adds no
dependency (`docs/architecture/algorik-blueprint-traceability.md:269,371-378`).
The contracts are listed above. The slice this record proposes is the
**posture-and-halt surface**: the portal's `system`, `loop`, `risk`,
`risk/limits` and `orders` pages, against `qip-web`'s `Overview`, `Risk`,
`Execution` surfaces and the console `Risk` view. Reasons:

- Both sides already carry the highest-consequence assertions — the paper
  label on every route, halt shown as halt, no order control, the kill switch
  trippable and never clearable — so parity is checkable against tests that
  exist rather than tests to be invented (`boundary.spec.ts`, `shell.spec.ts`,
  `console.rs:564-692`, `web.rs:87-163`).
- It reads only; the API endpoints it needs exist under `Viewer`.
- It does not touch identity. Identity is the hard slice and should be its
  own, second, with ADR 0019 in hand.

Not the marketing pages first: they prove nothing about the platform. Not
auth first: it is the riskiest and the least covered on the Rust side.

## Cutover evidence, as the owner's brief demands it

Parity, then retire. Stated so it can be checked rather than asserted:

1. **A parity harness that drives both surfaces from one fixture.** One JSON
   fixture of platform state, rendered by the Next.js page and by the
   `qip-web` surface, and the facts compared — every number, the posture
   label, the absent-versus-zero distinction, the halt state. A page that
   renders a number the other does not is a finding, not a formatting
   difference.
2. **Every Playwright test named above has a Rust-side counterpart by name, or
   a written reason it does not apply.** The counterpart is a test against the
   rendered HTML for rendering properties, and a browser-driven test for
   behaviour a person would notice (the frontend rule's own criterion).
3. **The security headers are equal or stricter.** The CSP in
   `http.rs:200-201` is the floor; Option B would have to state where it
   relaxes it and why.
4. **`PAPER TRADING` on every route, including error and offline pages**, by
   test, on the Rust side, before any TypeScript route is removed.
5. **Retirement is a deletion after the parity run is green in CI on the same
   commit**, one route group at a time, with ADR 0010's correspondence tests
   updated in the same change. Nothing is deleted on the strength of a
   deployment observation, because no deployment of either surface is
   observable from this tree (fact 2 above).
6. **The Phase 13 push drill** (`blueprint.md:3337`) is out of reach of
   this record: it needs a deployed PWA, Web Push, and thirty days. It is the
   blueprint's exit criterion, not this cutover's.

## Recommendation — marked as a recommendation, not a decision

**Option C.** Make `qip-web`, served by `qip-api`, the canonical Rust
first-party boundary now; take the posture-and-halt slice first with the
parity harness above; and pre-classify Leptos as a reversal of ADR 0001 and
ADR 0012 rather than a dependency request, admissible only against a measured
need for client-side state.

Why, in the order the reasons weigh:

- **The blueprint's outcomes are already met by the surface that exists, and
  its named mechanism costs two settled refusals.** Rust, one set of types,
  server-side SVG, no JavaScript framework, a CSP that forbids script: all
  present in `qip-web`. Leptos with hydration buys client interactivity that
  no requirement in the tree has asked for, at the price of an async runtime
  and `wasm-bindgen` in a deployed process.
- **ADR 0012's test refuses it today, and the test is the policy.** A web
  framework fails loudly. Admitting one means either abandoning the test or
  arguing it does not apply to the browser layer, and the second argument
  should be written by the owner in an ADR, not inferred by an agent.
- **The migration's hard part is identity, not rendering**, and identity is
  the same work under A, B or C. Choosing the rendering mechanism does not
  shorten it; choosing the one with no dependency does not lengthen it.
- **It leaves the door open honestly.** ADR 0001 wrote its own reversal
  condition. If the strategy marketplace or the explanation surface (§40.1,
  §40.2) turns out to need client-side state, that is the measurement, and
  Option B is then argued from evidence rather than from the blueprint's
  noun.

The honest cost of this recommendation: the traceability row for §40 stays
CONTRADICTS-by-name, and the owner may reasonably value the blueprint's name
over its outcomes. If so, Option B, with the superseding records it needs.

## What it costs

Each option's cost is stated beside it above; this is the sum for the
recommended one. Option C costs the owner an unresolved name: the
architecture of record says Leptos and the tree will say "Rust,
server-rendered; Leptos deferred", so every traceability row citing §40 stays
PARTIAL rather than ALIGNED until the reversal condition is either met or
declined by a later record. It costs the console team weeks of rewriting
thirty-four pages plus session issuance into `qip-web` with a Playwright-shaped
parity harness that does not yet exist, and every interaction stays a round
trip until a measured need for client-side state is shown. It costs nothing
in dependencies: `check-dependencies.sh` stays at eleven, and no async
runtime enters a process that serves posture. Option B would cost the
opposite — two settled refusals reopened and a transitive tree in the process
that renders `PAPER TRADING` — and that is why it is not recommended.

## What would make this wrong

- **A requirement for client-side state arrives.** A live blotter refreshing
  many times a second, a strategy marketplace that filters and sorts without
  a round trip, an explanation view that walks a causal graph interactively.
  Any of these is ADR 0001's reversal condition, and Option C says Leptos is
  then argued from it.
- **The push drill cannot be met from a hand-written service worker.** If
  Web Push and WebAuthn on the installed PWA need more than a small script —
  or if the owner will not accept any JavaScript, framework or not — the
  PWA half of §40 is unreachable under A and C, and the record must say so.
- **The parity harness cannot be built** because the two surfaces read
  different facts. That would be evidence that the Next.js portal renders
  things the API does not serve, which is a finding about the portal
  (`frontend/portal/CLAUDE.md` permits labelled simulated data), and the
  migration would first have to decide which of those pages are product and
  which are illustration.
- **The owner wants the blueprint by name.** Then Option B, and this record's
  contribution is the list of what B supersedes.

## What this does not decide

- It does not add, permit, or pre-approve any crate. `check-dependencies.sh`
  and `architecture.rs:1146` are unchanged by it.
- It does not amend ADR 0001, 0012 or the boundaries rule. Option B would
  need records that do; this one names them.
- It does not authorise deleting a TypeScript route, page, package or test.
  ADR 0022: transitional does not mean unmaintained.
- It does not decide where the session layer lives in Rust (`qip-api` or a
  new `apps/` crate), only that it is the second slice and its own decision.
- It does not touch the paper-trading boundary. No option here creates,
  eases or implies an order path; the frontend rule's prohibition on any
  control that could submit an order applies to the Rust surface exactly as
  to the TypeScript one, and `boundary.spec.ts:21` must have a Rust
  counterpart before it is retired.
- It does not decide the Phase 13 push drill or the mobile channel beyond
  noting that the portal PWA is the phone app today (`frontend/mobile/README.md`).

## Dependency-direction argument

Under every option the new or extended code is an `apps/` crate or `qip-web`,
which sits at the top of the graph. `qip-web` has no in-workspace edge
(`api_boundary.rs:298-301`) and must keep none; `qip-api` depends inward on
`qip-kernel` and below. No lib gains a dependency on a service, no service on
the runtime, and nothing depends on an app. Option B changes the count of
third-party crates at the top of the graph, not the direction of any edge in
it.
