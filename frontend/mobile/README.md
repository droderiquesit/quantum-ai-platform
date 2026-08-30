# frontend/mobile/

The mobile channel. Algorik ships on phones as the **installed PWA of the
portal** — one codebase, one authentication flow, one design system — rather
than as a separate native application. That is a product decision, not an
omission: the portal and the phone app must look identical and sign in
identically, and two codebases drift.

What the mobile experience actually consists of, and where it lives:

| Concern | Where |
|---|---|
| Web app manifest (name, icons, display mode) | `frontend/portal/src/app/manifest.ts` |
| Service worker (offline shell, never caches `/api/*`) | `frontend/portal/public/sw.js` |
| Install icons, maskables | `frontend/portal/public/icons/` |
| Install affordance in the UI | `frontend/portal/src/components/chrome/InstallApp.tsx` |
| Phone navigation (tab bar, off-canvas menu) | `frontend/portal/src/components/chrome/AppShell.tsx` |
| Phone-width behaviour tests | `frontend/portal/tests/` (Playwright mobile projects) |

This directory is the home for the artefacts that are mobile-*distribution*
specific and nothing else: a Trusted Web Activity / store-packaging project,
store listing assets, or platform signing configuration, when the desk decides
to list in a store. Until such an artefact exists, the directory holds only
this file — the PWA needs no packaging to be installable from the browser.

The licensed Cryptrix template that informed the mobile layout is a reference
asset in `vendor/templates/cryptrix` (ADR 0015).
