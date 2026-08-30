# frontend/

Next.js + TypeScript. **The commands here are not the workspace's** — nothing
in this directory is built or tested by `cargo`, and `make check` does not
cover it.

```
npm run lint      # eslint
npm run build     # type-check + production build; the real gate
npm run dev       # local dev server
npx playwright test
```

Rules: `.claude/rules/domains/frontend.md`.

Constraints worth repeating where they will be read:

- The browser holds no trading or risk logic. It renders what the platform
  decided; it never decides.
- **No control may submit an order**, and any posture display must carry the
  `PAPER TRADING` label. There is no live path and the UI must not imply one.
- **Themes re-value tokens, never components.** Dark is default; light lives
  entirely under `:root[data-theme="light"]` in `globals.css`, applied before
  first paint by the boot script in `layout.tsx`. A component that knows which
  theme it is in is a bug waiting for the other theme.
- **Simulated data is deterministic and labelled, or it does not exist.**
  Pages whose subsystem has no platform surface may illustrate it only through
  `src/lib/sim` (seeded, identical every load) under `SimulatedBanner`
  (`tests/theme.spec.ts` enforces the label). Fictional instrument names only;
  never simulate money the desk owns. A real figure and a simulated one never
  share a panel unmarked.
- The environment badge (`EnvironmentBadge`) is a report of what the platform
  and deployment declare — simulation blue, paper purple, staging amber,
  live-capable red. It is never a control, and red is an alarm, not a mode.

`node_modules/` is not committed. Check `git status` before staging here.
