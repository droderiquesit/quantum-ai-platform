# frontend/

The frontend domain: everything a browser runs, and nothing else (ADR 0016).
This directory is also the npm workspace root — `package.json` here governs
the portal and the shared packages; the Rust workspace is governed by
`backend/Cargo.toml`.

| Path | What it is | Gates |
|---|---|---|
| `portal/` | The authenticated console and installed PWA. Its own `CLAUDE.md` carries the binding rules. | `npm run lint && npm run build && npx playwright test` in `portal/` |
| `landing/` | The public site — the front door into the portal's sign-in. | `npm run build && npx playwright test` in `landing/` |
| `mobile/` | The mobile channel. The phone app **is** the portal PWA — see `mobile/README.md` for what lives where and why there is no second codebase. | Covered by the portal's mobile Playwright projects |
| `packages/` | Shared browser packages (`@algorik/brand`, `@algorik/design-tokens`, UI, API client, auth). Workspace members, portal-consumed. | Portal gates exercise them |
| `scripts/` | Frontend tooling — `generate-theme-css.mjs` emits the portal's token CSS; `npm run tokens:check` fails on drift. | `npm run tokens:check` here |

The landing keeps its own dependency tree deliberately — different React
major, licensed-template lineage per ADR 0015 — so it is *not* a workspace
member. Licensed template *packages* are reference assets and live in
`/vendor/templates`, never here.
