# Algorik programme — instruction precedence and discovery record

Produced before implementation, as the Algorik brief requires. It records what
instructions exist, which wins when two disagree, and what the repository
actually contained on the day the programme started. Written once so that the
agents working the slices do not each re-derive it — and so a reader can check
a later claim against what was true here.

Discovery run on branch `claude/autonomous-investment-platform-76gt4y`,
working tree clean.

## 1. What was found

| Kind | Location | Notes |
|---|---|---|
| Root instructions | `CLAUDE.md` | Working agreement; imports the four rule files below |
| Domain rules | `.claude/rules/{00-enterprise-governance,01-security-and-safety,02-change-management,10-product-direction}.md` | Governance, safety, change management, product direction |
| Architecture rules | `.claude/rules/architecture/{00-boundaries,01-testing-strategy}.md` | Dependency direction; test placement and mutation duty |
| Domain rules | `.claude/rules/domains/*.md` | core-rust, data-and-streaming, frontend, infrastructure, observability, risk-and-execution |
| Nested instructions | `frontend/CLAUDE.md`, `infrastructure/CLAUDE.md` | Closest-to-file conventions |
| Claude config | `.claude/settings.json` | Permissions, deny-list, two hooks |
| Agents | `.claude/agents/` — 13 agents | Reused, not duplicated (see §4) |
| Skills | `.claude/skills/` — 8 skills | `vision-to-plan` used to produce this plan |
| Hooks | `.claude/hooks/{guard-dangerous-command,format-rust-after-edit}.py` | Preserved untouched |
| ADRs | `docs/adr/0001`–`0012` | 0002, 0009, 0012 govern dependencies; 0003 paper trading |
| Commands | *(none — no `.claude/commands/`)* | Nothing to preserve |
| `AGENTS.md` | *(none in-repo; only inside `node_modules`)* | Nothing to preserve |
| `settings.local.json` | *(absent)* | Nothing to preserve |

Nothing in `.claude/` was overwritten, removed or consolidated: no duplicate
agents, no conflicting skills and no `commands/` directory were found, so the
brief's "consolidate duplicates and repair conflicts" step had nothing to act
on. That is a finding, not a skipped step.

## 2. Precedence, as applied

1. **Explicit user outcome and constraints** — the Algorik brief.
2. **Repository security and safety rules** — `.claude/rules/01-security-and-safety.md`
   and `00-enterprise-governance.md`.
3. **Root Claude configuration** — `CLAUDE.md`, `.claude/settings.json`.
4. **Domain-level configuration** — `.claude/rules/domains/*`, `architecture/*`.
5. **Application-level instructions** — `frontend/CLAUDE.md`, `infrastructure/CLAUDE.md`.
6. **Package-level conventions** — per-crate and per-package docs.

The brief states it supersedes conflicting *frontend, authentication,
repository-configuration and initial-hosting* instructions from the master
prompt, and retains everything else. It does not, and cannot, supersede rank 2:
a user instruction to weaken the paper-trading boundary or commit a secret is
refused by `01-security-and-safety.md` regardless of where it appears.

## 3. Conflicts found, and how each is resolved

Each is recorded here rather than silently resolved. Two produced ADRs.

### C1 — Requested npm packages vs. the dependency policy

The brief names Zod, TanStack Query/Table, React Router, Storybook, an
analytics SDK and a feature-flag SDK. `.claude/rules/domains/frontend.md`
governs `package.json` the way `Cargo.toml` is governed, and **ADR 0012**
admits a dependency only where all three hold: getting it wrong is silent, the
problem is adversarial or specialist, and a mature audited implementation
exists whose maintenance is somebody's job.

Applying that test honestly:

| Requested | Verdict | Reason |
|---|---|---|
| Zod | **not admitted** | Schema mismatches fail loudly; the shapes are already typed against the platform's own route table |
| TanStack Query | **not admitted** | `useResource` already models the states that matter here, including the four failure states a query library does not distinguish |
| TanStack Table | **not admitted** | The tables are dense but static-schema; no virtualisation need is measured yet |
| React Router | **not admitted** | Next's App Router is the router; adding a second is a defect |
| Storybook | **not admitted** | Fails all three conditions; a local component workbench route delivers the same review surface at no supply-chain cost |
| Analytics / feature-flag SDKs | **not admitted** | Thin abstractions written in-tree; the brief itself asks for an *abstraction*, which is what is built |
| **Identity token verification** | **admitted, by ADR 0013** | Passes all three: silent when wrong, adversarial and specialist, mature audited implementations exist |

Resolution: the shared packages are built with **zero runtime dependencies**,
which is what makes them shareable across web and a future React Native target
anyway. The one admitted dependency is argued in ADR 0013 and is confined to
backend token verification.

### C2 — "Live trading activation" vs. the paper-trading boundary

The brief lists live-trading activation among the actions requiring backend
authorization, and asks for a red `live` environment colour.
`.claude/rules/01-security-and-safety.md` forbids creating, enabling or easing
any live-order path, and forbids the UI implying one exists.

Resolution: the **authorization interface** is defined (every high-risk action
routes through one backend-authorised path with reason, reauthentication and
audit), and live-trading activation is **not implemented as a reachable
action**. The environment system renders all four colours, and `live` renders
as an *alarm* — the platform reporting a capability it must not have — never as
a mode an operator selects. Recorded as ADR 0014.

### C3 — Repository identity: "Algorik" vs. what is in the tree

`grep -rni algorik` over the whole repository returns nothing. There is no
Algorik brand package, logo, font, icon set, landing page, admin application,
mobile application or licensed template in the tree. The existing frontend is a
single Next.js console, currently branded PEOS Quantum AI, with 11 permitted
packages and 22 passing Playwright tests.

**Superseded by commit `c12b98f`.** The licensed package arrived while this
programme was in its second slice: the SignalAIX admin template
(`frontend/admin/`), a landing template (`frontend/landing/`), the Cryptrix
mobile PWA (`frontend/mobile/`), and the real Algorik brand assets
(`frontend/logos/`) — horizontal logo, icon set, favicon, Apple touch icon,
Android Chrome icons and a `site.webmanifest` declaring `#071B4D`.

The resolution above is therefore **withdrawn**. The brand is not authored
in-tree: `packages/brand` now serves the supplied artwork, and
`packages/design-tokens` derives its colours from the shipped icon — navy
`#071B4D`, cyan `#00c3fd`, blue `#005df8`, violet `#3700db`, sampled rather
than chosen. The invented "aperture" mark written under the old assumption has
been deleted.

The rest of §5's inventory stands: the console's component system, chart
primitives, API client, state components, hooks and tests are reused as
recorded, and were preserved through the merge that brought the templates in.

### C4 — Monorepo move vs. preserving working, tested code

The brief asks for root-level `packages/`. The existing app lives at
`frontend/`, has no CI workflow referencing it and no Kubernetes manifest
deploying it, but does carry the only passing behavioural suite in the
repository.

Resolution: `packages/` is created at the root as npm workspaces and the
existing app **consumes** them in place rather than being moved. A move buys
nothing here and risks the one tested surface. Recorded in ADR 0014.

## 4. Agents and skills — reused, not duplicated

The brief names nine specialist capabilities. Eight already exist and are
reused by their existing names; none was recreated under a new name.

| Brief's capability | Existing agent |
|---|---|
| Frontend developer | `frontend-engineer` |
| Product designer / UI-UX designer | `ux-designer`, `product-manager` |
| Mobile developer | `frontend-engineer` (the mobile surface is the installed PWA — see ADR 0014) |
| Accessibility reviewer | `ux-designer` + automated checks in the suite |
| Google Cloud architect | `cloud-platform-engineer` |
| Identity and security engineer | `security-engineer` |
| Backend integration engineer | `backend-engineer` |
| Test and verification engineer | `test-engineer` |

Concurrency is capped at three including the orchestrator, per the brief.

## 5. Existing-resource inventory

Classification per the brief: reuse unchanged · move to shared package ·
refactor · replace · remove as duplicate · retain during migration.

| Resource | Path | Classification |
|---|---|---|
| Design tokens | `frontend/src/app/globals.css` `@theme` | **move to shared package** (`design-tokens`) |
| Component primitives | `components/data/{Panel,Bits,Kpi,States,Simulated}.tsx` | **move to shared package** (`ui`) |
| Chart primitives | `components/viz/primitives.tsx` | **move to shared package** (`charts`) |
| API client + types | `lib/api/{client,types,endpoints}.ts` | **move to shared package** (`api-client`, `shared-types`) |
| Hooks | `lib/hooks/{useResource,useEventStream,connections,useSeries,useNow}.ts` | **move to shared package** (`api-client`) |
| Formatting | `lib/format/index.ts` | **move to shared package** (`ui`) |
| Simulation framework | `lib/sim/index.ts` | **move to shared package** (`testing`) |
| Gateway (BFF) | `app/api/gateway`, `lib/server/upstream.ts` | **refactor** — becomes the session-aware BFF |
| Chrome | `components/chrome/*` | **refactor** — shell splits into shared and app-specific |
| Console pages (35) | `frontend/src/app/**` | **reuse unchanged** — become the portal |
| PWA (manifest, worker, icons) | `frontend/public/*`, `app/manifest.ts` | **reuse unchanged** — the mobile surface |
| Playwright suite (22 tests) | `frontend/tests/*` | **reuse unchanged**, extended |
| Brand assets | — | **absent; authored new** |
| Licensed template | — | **absent** (see C3) |

## 6. Credential checkpoints — where this programme genuinely stops

Everything local and infrastructure-as-code is done first. These are the points
at which no further honest progress is possible without input:

1. **GCP bootstrap** — organization/billing/project, regions, Identity
   Platform enablement. Blocks deployment and blocks reporting any real URL.
2. **OAuth client** — consent screen, client ID, exact redirect URIs. Blocks
   real Google sign-in; the adapter and its contract are written regardless.
3. **DNS for `algorik.ai`** — registrar, nameserver authority, existing mail
   records. Blocks domain mapping; the migration runbook is written regardless.
4. **Apple/Google developer accounts** — blocks store distribution only. The
   installable PWA needs none of it.

No URL is reported as deployed until a deployment returns it.
