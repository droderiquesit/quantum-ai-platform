# Domain: frontend

**Scope** — `frontend/**`

Next.js + TypeScript, and the one part of this platform that is not Rust. The
exception is deliberate; ADR 0001 covers why everything else is.

## Approved

- TypeScript with `strict` on. Server components by default.
- Data arrives from `qip-api` over REST and SSE. The browser holds no trading
  logic and no risk logic — it renders what the platform decided.
- Playwright for behaviour a person would notice.

## Prohibited

- **Any control that could submit an order.** No live path exists and the UI
  must not imply one does.
- Rendering posture without the `PAPER TRADING` label.
- Reading a secret. The browser receives nothing the public may not see.
- Adding a dependency without review. `package.json` is governed like
  `Cargo.toml` and the transitive tree is part of the diff.

## Required evidence

`npm run lint` and `npm run build` in `frontend/`, plus Playwright output for
behavioural change. Say which ran.
