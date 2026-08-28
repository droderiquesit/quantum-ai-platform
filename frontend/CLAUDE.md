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

Two constraints worth repeating where they will be read:

- The browser holds no trading or risk logic. It renders what the platform
  decided; it never decides.
- **No control may submit an order**, and any posture display must carry the
  `PAPER TRADING` label. There is no live path and the UI must not imply one.

`node_modules/` is not committed. Check `git status` before staging here.
