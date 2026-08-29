export interface NavItem {
  readonly href: string;
  readonly label: string;
  /** Two or three characters for the collapsed rail. */
  readonly mark: string;
  readonly description: string;
  /** The platform surfaces this page reads, shown in the palette. */
  readonly reads: readonly string[];
}

export interface NavGroup {
  readonly label: string;
  readonly items: readonly NavItem[];
}

/**
 * The console's map: eight sections, desk-shaped.
 *
 * Grouped the way a research desk is organised rather than the way the API is —
 * what the loop is doing, what it found, what it decided, what it holds, what
 * could stop it, what it is learning, and what is running underneath.
 *
 * Every entry here reads at least one endpoint the platform actually serves.
 * That is the rule the section list is built against, and it is why there is no
 * economic calendar, no news-sentiment score, no bot marketplace and no
 * billing: this platform has no data behind any of them, and a page of invented
 * numbers is worse than an absent one — it is the same failure as a control
 * wired to nothing, which this codebase has now produced nine times.
 */
export const NAV: readonly NavGroup[] = [
  {
    label: "Desk",
    items: [
      {
        href: "/",
        label: "Command overview",
        mark: "OV",
        description: "Posture, the loop's cadence, and everything moving right now",
        reads: ["/health", "/system/status", "/system/metrics", "/portfolio", "/risk"],
      },
    ],
  },
  {
    label: "Intelligence",
    items: [
      {
        href: "/signals",
        label: "Live signals",
        mark: "SG",
        description: "The opportunity queue, proposals, and the signal stream",
        reads: ["/opportunities", "/proposals", "/stream/signals"],
      },
      {
        href: "/loop",
        label: "The eight-stage loop",
        mark: "LP",
        description: "What each stage produced, and what it refused, per cycle",
        reads: ["/cycle", "/system/metrics"],
      },
    ],
  },
  {
    label: "Market",
    items: [
      {
        href: "/markets",
        label: "Market state",
        mark: "MK",
        description: "Live market feed, instruments and the reference universe",
        reads: ["/stream/market", "/markets", "/assets", "/arbitrage"],
      },
      {
        href: "/data-sources",
        label: "Data sources",
        mark: "DS",
        description: "Source registry, latency, freshness, quality and licensing",
        reads: ["/data-sources", "/regions", "/mesh"],
      },
    ],
  },
  {
    label: "Book",
    items: [
      {
        href: "/portfolio",
        label: "Portfolio & P&L",
        mark: "PF",
        description: "Positions, cash, exposure and attribution",
        reads: ["/portfolio", "/capital", "/pnl", "/stream/positions"],
      },
      {
        href: "/orders",
        label: "Order blotter",
        mark: "OB",
        description: "Order lifecycle, fills, refusals and reconciliation",
        reads: ["/orders", "/fills", "/stream/orders"],
      },
      {
        href: "/order-entry",
        label: "Paper order entry",
        mark: "OE",
        description: "Stage a simulated ticket and see what the platform answers",
        reads: ["/system/status", "/risk"],
      },
    ],
  },
  {
    label: "Risk",
    items: [
      {
        href: "/risk",
        label: "Risk analyser",
        mark: "RK",
        description: "Exposure, concentration, tail risk, limits and the kill switch",
        reads: ["/risk", "/capital", "/autonomy"],
      },
      {
        href: "/capital",
        label: "Capital envelopes",
        mark: "CP",
        description: "Bounds, grants issued, utilisation and outstanding recalls",
        reads: ["/capital", "/regions"],
      },
    ],
  },
  {
    label: "Research",
    items: [
      {
        href: "/strategies",
        label: "Strategy ladder",
        mark: "ST",
        description: "Candidates, their rung, and the evidence each one stands on",
        reads: ["/strategies"],
      },
      {
        href: "/models",
        label: "Models & compute",
        mark: "ML",
        description: "Model registry, routing rungs, quantum jobs and their baselines",
        reads: ["/models", "/quantum", "/training"],
      },
    ],
  },
  {
    label: "Automation",
    items: [
      {
        href: "/agents",
        label: "Agent roster",
        mark: "AG",
        description: "Every agent, its grants, and the governance review of the roster",
        reads: ["/agents", "/system/governance"],
      },
    ],
  },
  {
    label: "Platform",
    items: [
      {
        href: "/system",
        label: "System topology",
        mark: "SY",
        description: "Services, mesh, edge cells and event-chain integrity",
        reads: ["/system", "/mesh", "/regions", "/stream/health"],
      },
      {
        href: "/integrations",
        label: "API & integrations",
        mark: "AP",
        description: "Every route this platform serves, its role, and what answers now",
        reads: ["/", "/openapi.json"],
      },
    ],
  },
];

export const NAV_ITEMS: readonly NavItem[] = NAV.flatMap((group) => group.items);

export function navItemFor(pathname: string): NavItem | undefined {
  return NAV_ITEMS.find((item) => item.href === pathname);
}
