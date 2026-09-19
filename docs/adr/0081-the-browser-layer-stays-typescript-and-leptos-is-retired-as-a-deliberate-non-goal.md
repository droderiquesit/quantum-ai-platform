# ADR 0081: The browser layer stays TypeScript, and Leptos is retired as a deliberate non-goal

- **Status**: Accepted, under the authority the owner delegated to the
  policy lane on 2026-09-19 over legal, business and policy decisions. That
  delegation is the only reason an agent may take a decision ADR 0025 said
  was the owner's; it is recorded here so that a reader can see the record
  was taken on delegated authority and not on an agent's own initiative.
- **Date**: 2026-09-19
- **Supersedes**: the open half of ADR 0025. Amends ADR 0022 item 4.
- **Related**: ADR 0001 (Rust for everything, including the web interface),
  ADR 0002 and ADR 0009 (two dependencies), ADR 0010 (`qip-web` is a library
  linked into `qip-api`), ADR 0011, ADR 0012 (a web framework fails the
  three-part test by name), ADR 0013 (a browser SDK refused for its
  transitive tree), ADR 0014 (one design system, four surfaces), ADR 0019
  (identity lives in the Next.js server), ADR 0025 (the Rust frontend
  boundary — its decided half stands verbatim)

## Context

The blueprint names Leptos in five places (`grep -n Leptos
docs/architecture/algorik-blueprint-v10.1-source.md` — §40's opening
sentence, §40.4, §40.5's layer table, §54.4's mobile row, and rule 71). ADR
0022 item 4 made it "the target experience layer" and declared
`frontend/portal` and `frontend/landing` transitional. ADR 0025 then split
the question: it **decided** that Leptos is a reversal of ADR 0001 and ADR
0012 rather than a dependency request under ADR 0009's edge tier, admissible
only by a record that supersedes both; and it **declined to decide** whether
the TypeScript portal is rewritten, because that was the owner's call. The
register has carried "ADR 0022/0025 | Leptos | A decision to authorise it"
in its blocked table since.

What exists, re-verified in this worktree rather than taken from ADR 0025,
because two of its figures had drifted:

- **No Leptos code anywhere.** A case-insensitive search of every `.rs`,
  `.toml`, `.ts`, `.tsx` and `.json` file in the repository finds no file.
  The register's CI-executed absence claim (`grep -rli leptos
  frontend/portal/src` returns nothing) is the one the build runs.
- **The portal has sixty-eight `page.tsx` files**, not the fifty-four ADR
  0025 counted: `find frontend/portal/src/app -name page.tsx | wc -l`. Nine
  under `(auth)`, eleven under `(marketing)`, forty-eight under `(portal)`.
- **Thirty-four Playwright spec files.** `grep -c '^\s*test(' frontend/portal/tests/*.spec.ts`
  summed to 149 on 2026-09-19; the lane brief said 162, which this count
  does not reproduce and which may include the landing's suite. Neither
  figure is load-bearing; what is load-bearing is that `boundary.spec.ts`,
  `shell.spec.ts`, `mobile.spec.ts` and `worker.spec.ts` exist and are run by
  `ci.yml` (`grep -n 'npx playwright test' .github/workflows/ci.yml`, twice:
  once for the portal, once for the landing).
- **The phone app is the portal PWA**, by product decision recorded in
  `frontend/mobile/README.md`: manifest at
  `frontend/portal/src/app/manifest.ts`, service worker at
  `frontend/portal/public/sw.js`, install affordance at
  `frontend/portal/src/components/chrome/InstallApp.tsx`. No native shell.
- **The Rust surface, `qip-web`, is a library** linked into `qip-api` and
  served under `Viewer` (ADR 0010). It has no in-workspace edge and cannot
  act except to trip the kill switch. It is not deployed anywhere the public
  can reach; the TypeScript surfaces were the ones observed serving before
  the 2026-09-13 teardown.
- **The two-dependency rule holds every crate**, not only the decision core:
  `scripts/check-dependencies.sh` lists eleven packages, all serde's
  closure, and
  `architecture.rs::no_crate_declares_a_third_party_dependency_beyond_the_two_permitted`
  holds the workspace to them.

### What §54.4 actually requires

Read from the section rather than from any row's paraphrase. The mobile row
says: "Installable Leptos PWA. No native shell. Web Push and WebAuthn cover
the two capabilities that justified native. One hundred percent Rust from one
codebase. Native shell only if the push drill misses target." Four of those
five clauses are **outcomes** — installable, no native shell, Web Push,
WebAuthn, one codebase — and one is a **name**. The outcomes are met or being
met by the surface that exists: the portal is installable from one codebase
with no native shell today, and the WebAuthn half is a separate lane this
wave (§40.3/§40.4, ADR 0038's owner-run checks), which this record does not
decide. The clause this record retires is the name and the sentence attached
to it, "one hundred percent Rust from one codebase".

The same section's frontend-graph row refuses a JavaScript charting
dependency because it would sit in "a platform whose supply chain argument
rests on one dependency graph". That is the platform's own argument, and it
cuts the other way from the mobile row: a Leptos application brings an async
server integration, `wasm-bindgen`, `js-sys`, `web-sys`, a build tool outside
cargo, and a transitive closure in the hundreds of crates — into the Rust
graph that is currently eleven packages, and into the process that renders
`PAPER TRADING`. ADR 0025 stated those as claims for the owner to verify
against a lockfile; nothing has changed them.

### The functional gain of a rewrite, named

The brief asked for one concrete functional gain or a statement that there
is none. There is one, and it is not enough.

**Shared Rust types across the browser boundary.** A Leptos portal would
deserialise `qip-api`'s response structs from the same definitions the API
serialises, so a renamed field breaks the build rather than a page. That is
real. Two facts reduce it: ADR 0025 found that "shared types with every
backend service" exists on neither side today (the portal's shared-types
package is two hand-written string unions; the Rust view model is
`qip-api`'s and `qip-web`'s, not every service's); and the same guarantee is
available without a rewrite, by generating the portal's request and response
types from `qip-contracts` through serde — a build step, no new crate, and a
diff a reviewer reads. If that generated contract proves insufficient, that is
a measurement and a reversal condition below.

Every other candidate gain dissolves on inspection. Client-side interactivity
is ADR 0001's own reversal condition and nothing in the tree has asked for it.
"Zero lines of JavaScript framework" is a supply-chain claim that the Leptos
transitive tree contradicts. The push drill and WebAuthn are browser APIs
called from a service worker and a page script in every framework including
Leptos; the framework does not remove the JavaScript, it wraps it.

### The cost of a rewrite, named

Sixty-eight pages, the session layer (sealed cookie, CSRF, the viewer-token
gateway — ADR 0019, all portal server code), thirty-four spec files to be
answered by name or by written reason, and a parity harness that does not
exist. ADR 0025 estimated "weeks of a team's work" against fifty-four pages;
it is more now. The hard part is identity, and identity is the same work
under every option — choosing the rendering mechanism does not shorten it.

## Decision

1. **Leptos is not authorised, and is retired as a deliberate non-goal.** No
   crate, no application, no build tool. The register scores §54.4's mobile
   row, §40's opening sentence, §40.5's layer table and rule 71 against
   their outcomes — installable PWA, one codebase, no native shell, Web Push,
   WebAuthn — and records the name as retired by this ADR. A retired clause is
   not `REACHED`; the rows stay `PARTIAL` and say which outcomes are met and
   which are not.

2. **The browser layer is TypeScript by decision, not by transition.** The
   one non-Rust exception ADR 0001, ADR 0011 and the product-direction rule
   already name is confirmed as the standing decision. ADR 0022 item 4 is
   amended: `frontend/portal` and `frontend/landing` are no longer
   "transitional", and "ADR 0001's browser exception is superseded in
   direction" is withdrawn — the exception stands as written.

3. **ADR 0025's open half is closed by its Option D**, the option it listed
   for completeness and did not argue for because the choice was then the
   owner's. Its decided half stands verbatim and becomes this record's
   reversal mechanism: any future Leptos request is a reopening of ADR 0001
   and ADR 0012, admissible only by a record that supersedes both and amends
   `.claude/rules/architecture/00-boundaries.md`'s "no new async runtime".

4. **`qip-web` is not extended toward parity with the portal.** It stays what
   ADR 0010 made it — the API's own server-rendered viewer surface, no
   JavaScript, `default-src 'none'` — and it stays the platform's fallback
   surface (see the reversal conditions). ADR 0025's first-slice and cutover
   plan is not executed; it is kept in that record as the shape a migration
   would take if a reversal condition fires.

5. **The two-dependency rule is not extended to the frontend and is not
   needed to be.** `package.json` is governed as `.claude/rules/domains/frontend.md`
   already says — every addition reviewed with its transitive tree in the
   diff — and no numeric cap is set here, because the honest position is that
   the browser layer has a dependency tree the platform can review but cannot
   hold to two, and pretending otherwise is the failure ADR 0009 named.

## Consequences

- The blocked-table row in `docs/DELIVERY-STATUS.md` moves from "a decision
  to authorise it" to "decided: not authorised". §54.4 and §56.7 rule 71 are
  re-worded in place without changing verdict.
- The CI-executed absence claim `grep -rli leptos frontend/portal/src`
  returns nothing stays in the register and keeps running, now as a guard
  on this decision rather than as a pending one.
- No file under `backend/`, `frontend/` or `infrastructure/` changes.
- ADR 0022's item 4 and ADR 0025's status line should be annotated in the
  index to point here; the bodies are left as the record of what was decided
  when, per the index's own rule on anachronistic rewrites.

## What it costs

**The blueprint stays contradicted by name in five places**, and the register
says so in each: §2.1's "every application is Rust", §40's "one Leptos
codebase", §40.5, §54.4's mobile row and rule 71. Retiring a name is cheaper
than a rewrite and it is still a departure from the architecture of record,
which ADR 0022 chose. This record pays that in the open.

**Two toolchains, two supply chains, for good.** The portal's dependency
tree is npm's (Next 16, React 19, `chart.js`, eleven workspace packages) and
the landing keeps its own. `scripts/check-dependencies.sh` stays at eleven
for the Rust workspace and there is no equivalent single number for the
browser layer. Every `npm audit` finding is a finding in a surface that
renders posture.

**Identity stays in the Next.js server.** The sealed cookie, CSRF and the
viewer-token gateway remain portal code (ADR 0019). ADR 0076 has already
found that the console cannot carry an operator assertion; nothing here
moves that either way, but a reader hoping a Rust portal would have solved
the carrier problem should note that it would not have — ADR 0025 said the
session layer is the hard slice under every option.

**The type contract between `qip-api` and the portal is still hand-written.**
Decision 1 names the generated-contract remedy and does not build it. Until
somebody does, a renamed field breaks a page and not a build, which is the
one concrete thing a rewrite would have bought.

**`qip-web` becomes a surface with no growth path.** It stays correct,
tested and served, and nobody is asked to extend it. That is a deliberate
asymmetry: a second portal grown in parallel would be the two-source-of-truth
failure with a browser in front of it.

## What would make this wrong

- **The owner wants the blueprint by name.** Then ADR 0025's Option B, with
  the records it lists: a supersession of ADR 0001 and ADR 0012's
  async-runtime refusal, an amendment to the boundaries rule, a widened
  `PERMITTED` list with a reason per crate, and the tier test relaxed for one
  named crate while the decision core stays at two. This record's
  contribution to that day is decision 3: the request is a reversal, not an
  addition.
- **The npm tree fails the platform's own gate with no upstream fix for a
  sustained period.** The managed-Prometheus sidecar was refused on exactly
  that shape (ADR 0026, an unfixed CRITICAL with no tag to move to). If the
  portal's tree reaches that state, the TypeScript surface becomes
  undeployable under the platform's own rules, and the fallback is `qip-web`
  — server-rendered, no JavaScript — **not** Leptos, because a Rust surface
  with a hundred-crate transitive tree has the same disease in a different
  package manager.
- **The generated type contract proves insufficient.** If `qip-contracts`
  serialised through serde cannot express what the portal needs, and the
  drift between API and page costs a real incident, that is the measurement
  ADR 0001 asked for and the argument for shared Rust types is then made from
  evidence.
- **The Phase 13 push drill fails for a reason attributable to the portal's
  service worker or manifest** (delivery below 99.5 percent, p99 over fifteen
  seconds over thirty days — §40.4). The blueprint's own answer is a native
  shell with view code only; this record does not pre-empt that, and notes
  that a native shell is not Leptos either.
- **A requirement for client-side state arrives** — the live blotter, the
  interactive causal walk. ADR 0001's reversal condition, met first by
  server-sent events (which `qip-api` speaks) and by the portal's own client
  components before any framework question is reopened.

## Alternatives considered

**Authorise Leptos and rewrite the portal (ADR 0025 Option B).** Rejected:
the only functional gain is shared types, obtainable without a rewrite; the
cost is two settled refusals reopened and a transitive tree in the process
that renders posture; and the blueprint's own supply-chain argument in §54.4
argues against it.

**Migrate the portal into `qip-web`, server-rendered, and retire the
TypeScript surfaces (ADR 0025 Options A and C).** Rejected for now: it takes
no dependency, which is why ADR 0025 recommended it, but it costs the same
weeks of identity and page work as a Leptos rewrite for an outcome the
platform does not need — the deployed, tested, installable surface is the
TypeScript one. It remains the fallback route above rather than the plan.

**Leave the question open, as ADR 0025 did.** Rejected: an open question in
the blocked table reads as work somebody will do, and three waves of lanes
have re-derived the same analysis. A non-goal with a reversal condition is
cheaper to carry than a pending decision.

## Dependency-direction argument

Nothing in this record adds, removes or redirects an edge in the Rust
workspace. `qip-web` keeps no in-workspace dependency
(`api_boundary.rs::the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate`);
`qip-api` depends inward on `qip-kernel` and below; no lib gains a dependency
on a service and no service on the runtime. The browser layer is outside the
Cargo graph entirely, which is the property this record preserves.
