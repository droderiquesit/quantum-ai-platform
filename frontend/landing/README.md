# frontend/landing — the public front door

The unauthenticated marketing site. Next.js App Router, JavaScript, its own
dependency tree (it is deliberately **not** an npm workspace member — see
`frontend/CLAUDE.md` and ADR 0015/0016). The visual system is the licensed
Fortradex template; the content is this platform's, and nothing the template
invented is allowed to survive in it.

```
npm run lint     # the checks below, no dependency required
npm run build    # next build (standalone output, what the container runs)
npm run start    # serve the build on :3500
npx playwright test   # the behavioural gate, desktop and phone
```

## Which surface owns which page

The portal carries a `(marketing)` route group with eleven pages, and this app
carries twelve. Two sites saying overlapping things about one company is how
they come to disagree, so the ownership is stated here rather than left to
whichever was edited last.

**The landing owns the public marketing surface.** It is the front door, it is
the origin a search engine and a regulator will find, and it is the only one of
the two that a visitor reaches without a session. Every public page lives here:

| Route | What it is |
|---|---|
| `/` | The front door: posture, coverage, the loop, the boundary |
| `/platform` | The eight stages, and what is served today versus in research |
| `/technology` | The reasoning panel, and quantum against its classical baseline |
| `/security` | The three-layer paper-trading boundary, the audit chain, secrets |
| `/institutional` | Reproducibility, attribution, capital control |
| `/developers` | The REST and SSE surface, and why external access is gated off |
| `/company` | What Algorik is, what it is not, and the operating principles |
| `/contact` | The one address, with an honest account of what answers |
| `/legal` | Index of the three documents |
| `/legal/risk-disclosures` | **Required.** Simulated ≠ live; no live order is submitted |
| `/legal/terms` | Draft terms of service |
| `/legal/privacy` | Draft privacy policy |

`/about` redirects to `/company` (308) and `/error` redirects to `/` (308).

The copy for the pages that exist on both surfaces is taken **word for word**
from the portal's `(marketing)` group rather than rewritten, so the two cannot
currently say different things. That is a holding position, not a design: the
duplication is real and one of the two must go.

**The open follow-up, for the portal's owner:** the portal's
`src/app/(marketing)/**` should become redirects to this origin, leaving the
portal to be the console it is named for. That change is outside this app's
paths and has not been made here.

## What `npm run lint` checks

`next lint` was removed in Next 16. The script that invoked it did not lint —
it failed with *"Invalid project directory provided, no such directory:
.../lint"* — and the landing's CI job never ran it, so nothing noticed. It is
now `scripts/lint.mjs`, which adds no dependency and enforces the defects this
site has already shipped:

- **Asset references must be absolute.** `src="assets/x.png"` resolves against
  the current route, so it is correct only while every route is one segment
  deep. The first nested route (`/legal/terms`) 404s every image on it. All 696
  of them were relative; a mutation restoring one is caught here *and* by the
  Playwright crawl.
- **Every asset named in source must exist** under `public/`.
- **Every internal `href` must be a route this app serves** — as a JSX
  attribute *and* as a data property, because the navigation is generated from
  `lib/site.js`.
- **No `.html` destinations, no `index-N` demo routes, no lorem, no vendor
  brand name.**
- **No `<form>`.** There is no verified delivery path from this site, and a
  form that accepts a message and drops it is worse than no form. It also means
  no control here could ever submit anything.
- **`class=` in JSX** — React ignores it and logs an error; the template's
  preloader did this on every route transition.
- **Only `NEXT_PUBLIC_ALGORIK_PORTAL_URL` may be read from the environment.**
  The browser receives nothing the public may not see.
- **The `PAPER TRADING` label must exist**, in those words.

ESLint proper is a separate, reviewable decision. `eslint` and
`eslint-config-next` are devDependencies of the portal and could be adopted
here too; that addition was not made unilaterally.

## Images

Every photographic asset in `public/assets/images/` is a grey placeholder with
its own pixel dimensions printed on it — the template ships dimension stand-ins,
not artwork. Only the shapes, icons and the brand files are real.

So the illustrations are drawn in the repository as SVG, in
`components/art/`: the eight-stage loop, the three-layer boundary, the hash
chain, the reasoning panel, the quantum-versus-baseline comparison, the
discard funnel, the regional cells, the console mock-up and the five coverage
marks. No dependency, crisp at any density, and nothing on them is a number a
reader could mistake for market data.

The lockup is the approved brand file copied from
`frontend/packages/brand/assets/`. The template's `logo.png` was the same
lockup flattened onto an opaque white plate, which rendered as a white
rectangle on the footer's grey ground.

## Where the styles live

`public/assets/css/**` is the licensed template's stylesheet and is treated as
reference material. Algorik's own rules are in `app/algorik.css` so they are
visible in a diff rather than buried in 34,000 lines of vendor CSS. That file
also carries the fixes for things the template left broken — the fonts the
`:root` asked for were never loaded, and the coverage cards revealed their body
copy only on hover, which on a touch screen means never.

## The rules this app is held to

`.claude/rules/domains/frontend.md`. In short: no control that could submit an
order; the `PAPER TRADING` label wherever posture is shown; no secret reaches
the browser; and `package.json` is governed like `Cargo.toml`.
