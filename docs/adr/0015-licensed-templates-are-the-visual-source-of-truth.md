# 0015 — The licensed templates are the visual source of truth

**Status:** accepted
**Supersedes** the visual-direction half of ADR 0014 (its package structure,
mobile-as-PWA and no-second-router decisions stand).

## Decision

The owner supplied the licensed template packages in-repo (`vendor/templates/signalaix` —
SignalAIX, `frontend/landing`, `vendor/templates/cryptrix`) and
directed, twice, that the product look like them. Using a purchased template
as the skin of one's own product is the licence's intended use, so the
surfaces now adopt the templates' own assets rather than an in-house
approximation:

- **Portal**: SignalAIX's theme tokens, component/behaviour CSS layer, shell
  structure (sidebar + top header) and page compositions, with this
  platform's live data bound where the template shows demo data. The
  template's stack (Tailwind v4 utilities + a small component layer) matches
  ours, which is what makes verbatim adoption of its classes work under our
  build.
- **Landing**: the Fortradex Next app is the landing surface's basis
  (integration tracked separately — it is a complete app with its own
  dependency tree).
- **Mobile**: Cryptrix styles the installed PWA screens (tracked separately).

`frontend/packages/design-tokens` remains the single mechanical source the stylesheet
is generated from, but its *values* are now SignalAIX's palette verbatim, and
the generator additionally emits the template's own token names
(`--color-bg`, `--color-panel`, `--color-text`, `--color-muted`,
`--color-accent`, …) so template markup and CSS run unmodified.

**Runtime dependency admitted: `chart.js`.** The template's charts are
Chart.js and pixel fidelity includes them. This is a user-directed admission
recorded here per the frontend dependency rule; it enters `package.json`
rather than a CDN tag so the build pins the version and deployment does not
depend on a third-party CDN being up. Lucide icons ship as the template's own
inline SVGs — no icon dependency.

## What does not move

The safety chrome is not template-negotiable: the PAPER TRADING banner on
every route, the environment badge as a report-not-control, the account menu
with server-side sign-out, the honest empty/absent/refused states, and the
SIMULATED banner on generated data. These render *inside* the template's
visual system, restyled to belong, never removed. Where the template names a
capability the platform does not have, the page states so in the template's
own visual idiom rather than showing its demo numbers as if real.

## What it costs

Two token vocabularies (ours semantic, theirs template) alias to one value
set — the generator keeps them from drifting, but readers meet both names.
The in-house chart primitives remain for pages not yet ported; two chart
systems coexist until the port completes. And the template's ~40 pages map
onto a platform with fewer real surfaces, so some template pages bind to
declared absences — which is the honest rendering of "the same look" over a
platform that does not fake data.

## What would make this wrong

If the licence for any package turns out not to cover this use, the affected
surface reverts to the in-house system (which remains in history at
`d3ab6a6`). If template CSS updates ever have to be hand-merged repeatedly,
the extraction should move to a scripted import.
