/**
 * A stubbed platform, at the browser boundary.
 *
 * Every route is intercepted with `page.route`, so the application code under
 * test is byte-for-byte the code a deployment runs: there is no mock data path
 * inside the app, and no test-only branch for the app to take.
 *
 * The gateway's disposition header is part of the contract, not decoration.
 * `client.ts` reads `x-qip-gateway` *before* it looks at the status code, so a
 * stub that returns 502 without the header is telling the console the platform
 * answered 502, which is a different fact from the platform being unreachable.
 * Getting that wrong would let a test pass while the console showed the wrong
 * failure to an operator during an incident.
 */
import type { Page, Route } from "@playwright/test";

export const GATEWAY = "**/api/gateway/**";

/** The disposition header the gateway sets, mirroring `GATEWAY_HEADER`. */
const HEADER = "x-qip-gateway";

/** Endpoint suffix → body. Keys are matched against the end of the path. */
export type Bodies = Readonly<Record<string, unknown>>;

/**
 * A platform that answers with `bodies`, and reports anything else as not wired.
 *
 * The catch-all mirrors what `qip-api` genuinely does: 8 of its 27 read
 * endpoints answer `200` with `{subject, available: false, reason}` because the
 * subsystem behind them is not composed into that process. Returning a 404
 * instead was measurably wrong &mdash; it put the console into a re-render loop
 * that detached chrome controls from the DOM, which is a state the real
 * platform never produces.
 */
export async function servePlatform(page: Page, bodies: Bodies): Promise<void> {
  await page.route(GATEWAY, async (route: Route) => {
    const path = new URL(route.request().url()).pathname.replace("/api/gateway", "");
    const key = Object.keys(bodies).find((k) => path === k || path.endsWith(k));
    if (key === undefined) {
      await route.fulfill({
        status: 200,
        headers: { [HEADER]: "upstream", "content-type": "application/json" },
        body: JSON.stringify({
          subject: path.replace(/^\//, ""),
          available: false,
          reason: `no stub for ${path} in this specification`,
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      headers: { [HEADER]: "upstream", "content-type": "application/json" },
      body: JSON.stringify(bodies[key]),
    });
  });
}

/** A platform nothing can reach, reported the way the real gateway reports it. */
export async function servePlatformUnreachable(page: Page): Promise<void> {
  await page.route(GATEWAY, async (route: Route) => {
    await route.fulfill({
      status: 502,
      headers: { [HEADER]: "unreachable", "content-type": "application/json" },
      body: JSON.stringify({ error: "the platform is not answering on 127.0.0.1:8080" }),
    });
  });
}

/**
 * The shapes the chrome reads on every route.
 *
 * Taken from a real `qip-api` process rather than invented: these are the
 * bodies `GET /api/v1/health` and `GET /api/v1/system/status` actually return.
 */
export function healthy(overrides: Record<string, unknown> = {}) {
  return {
    "/health": {
      status: "ok",
      halted: false,
      autonomy: "paper_trading",
      live_capable: false,
      reconciliation_breaks: 0,
      ...overrides,
    },
    "/system/status": {
      autonomy: "paper_trading",
      configured_autonomy: "paper_trading",
      ceiling: "paper_trading",
      live_capable: false,
      halted: false,
      halted_scopes: [],
      cycles: 12,
      events: 12,
      archived: 12,
      mesh: { served: false },
      ...overrides,
    },
  };
}

/**
 * `GET /api/v1/risk`, copied verbatim from a running `qip-api`.
 *
 * Verbatim and not hand-written, because a hand-written one was wrong in a way
 * that was hard to see: it carried `exposure` and `kill_switch` and omitted
 * `concentrations`, `limit_utilisation` and `tail_risk`, and the console
 * crashed to its global error page rather than rendering. A fixture that is a
 * plausible shape rather than the real one tests the console against a platform
 * that does not exist.
 */
export const RISK_BODY = {
    "exposure": {
      "subject": "exposure",
      "available": false,
      "reason": "no edge cell has reported to this process. The central plane's view of a cell comes from a CellReport the cell pushes; until one arrives there is no book, no utilisation and no latency to show. This is a silent feed, not a flat one."
    },
    "concentrations": {
      "subject": "concentrations",
      "available": false,
      "reason": "no edge cell has reported to this process. The central plane's view of a cell comes from a CellReport the cell pushes; until one arrives there is no book, no utilisation and no latency to show. This is a silent feed, not a flat one."
    },
    "kill_switch": {
      "halted": false,
      "halted_scopes": [],
      "tripped_by": "",
      "reason": "",
      "clearances": 0
    },
    "limit_utilisation": {
      "subject": "limits",
      "available": false,
      "reason": "the configured limit set is held by the risk monitor and its live utilisation by the desk's capability-gated risk view. Neither is reachable from an HTTP handler, so this shows no limits rather than showing every limit at zero per cent \u2014 which would read as headroom the platform has not measured."
    },
    "tail_risk": {
      "subject": "tail_risk",
      "available": false,
      "reason": "the desk holds this behind a capability gate. This process serves HTTP requests under no agent identity, so it cannot pass one, and it does not reach past a control to fill in a panel. Read it through an agent run."
    }
  } as const;
