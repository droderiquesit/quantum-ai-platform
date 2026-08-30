# 0014 — One design system, four surfaces, no second router

**Status:** accepted

## Decision

Algorik's landing site, customer portal, administrative surface and mobile
application derive from one set of shared packages with **no runtime
dependencies**:

```
packages/brand          logos, wordmark, the mark's rules
packages/design-tokens  colour, type, space, radius, elevation, motion
packages/ui             accessible primitives over those tokens
packages/charts         financial chart primitives (hand-drawn SVG)
packages/auth           provider abstraction, session state, guards
packages/api-client     typed client and hooks for the platform's surface
packages/shared-types   the platform's response shapes
packages/validation     schema helpers, in-tree
packages/analytics      privacy-aware abstraction, no vendor
packages/feature-flags  typed flags
packages/testing        fixtures, deterministic simulation, render helpers
```

Three structural choices, each with a reason:

**The existing app consumes the packages in place; it is not moved.** The
console at `frontend/` is the only surface in this repository with a passing
behavioural suite. No CI workflow references it and no manifest deploys it, so
a move buys no cleanliness, and it would put 22 passing tests through a path
rewrite for aesthetics. `packages/` is created at the root as npm workspaces
and the app depends on them where it stands.

**Mobile is the installed progressive web app, not a second codebase.** The
brief asks for shared brand, terminology, secure authentication, deep links and
biometric reauthentication — all of which a PWA provides, and all of which a
second native codebase would provide *twice*, including twice the places for the
paper-trading guarantee to drift. A React Native target may later consume
`design-tokens`, `shared-types`, `validation` and `api-client` unchanged, which
is why those four are written free of DOM assumptions. Store distribution needs
Apple and Google accounts the programme does not have; installability needs
neither.

**There is no second router, no second data layer and no component-workbench
vendor.** Next's App Router routes; `useResource` fetches; the workbench is a
route in the portal that renders every component in both themes at the tested
viewports. Each of these replaces a package the brief named, and each refusal is
argued in ADR 0013 against ADR 0012's three conditions.

## The environment system, and the one thing it may not become

Four environments render as four colours — simulation blue, paper purple,
staging amber, live red — on every surface, from one token set. The brief lists
live-trading activation among the actions requiring backend authorization.
`.claude/rules/01-security-and-safety.md` forbids creating, enabling or easing
a live-order path and forbids the UI implying one exists.

Both are satisfied precisely: the **authorization interface** is real and every
high-risk action routes through it — reauthentication, typed reason, backend
decision, audit event, and dual approval where required. Live-trading
activation is not among the actions that interface can reach. The `live` colour
renders only when the platform itself reports a live capability, and it renders
as an alarm to be investigated, never as a mode an operator selects. A control
that could turn it on does not exist, and the Playwright suite asserts the
environment indicator is inert markup rather than a button.

## What it costs

**Two package boundaries to maintain.** A component that belongs in `ui` will
be written in an app first, and someone has to move it. The workbench route is
the pressure that makes the omission visible.

**No vendor ecosystem.** No community table virtualiser, no query-cache
devtools, no Storybook addons. When a measured need appears — a table that is
genuinely too large to render, a cache invalidation the hooks cannot express —
that measurement is the argument for a dependency, and it goes through ADR
0012's three conditions like anything else.

**Zero-dependency packages are more code to own.** That is the same trade the
repository has already made deliberately, and the same reason it holds: the
supply chain of the code that renders a position is small enough to read.

## What would make this wrong

* If a React Native target is genuinely required for store distribution, the
  PWA decision is reopened — but the four platform-neutral packages are the
  reason that reopening is a port and not a rewrite.
* If the shared packages accumulate app-specific branching, the boundary is not
  earning its cost and the honest response is to collapse it rather than to
  keep adding flags.
