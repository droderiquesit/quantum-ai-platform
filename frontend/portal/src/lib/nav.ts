export interface NavItem {
  readonly href: string;
  readonly label: string;
  /** Two or three characters for the collapsed rail. */
  readonly mark: string;
  readonly description: string;
  /** The platform surfaces this page reads, shown in the palette. */
  readonly reads: readonly string[];
  /** True where the page's figures are a labelled deterministic illustration. */
  readonly simulated?: boolean;
}

export interface NavGroup {
  readonly label: string;
  readonly items: readonly NavItem[];
}

/**
 * The console's map: the eight PEOS sections.
 *
 * Every entry resolves to a page, and every page is one of three honest
 * things: a client of an endpoint the platform serves; a rendering of the
 * platform's own `available: false` reason; or a deterministic illustration
 * labelled SIMULATED DATA and written against the typed contract the real
 * surface will have. Nothing here invents a live number, and nothing is a
 * dead link kept to make a section look finished.
 *
 * Trading & Execution reads the platform's execution record and nothing more.
 * There is no ticket entry here and no entry point to one: this console is a
 * capital control surface, not a trading terminal, and a section that offered
 * to compose an order would imply a path that does not exist.
 */
export const NAV: readonly NavGroup[] = [
  {
    label: "Command Center",
    items: [
      {
        href: "/",
        label: "Executive dashboard",
        mark: "EX",
        description: "Posture, the loop's cadence, and everything moving right now",
        reads: ["/health", "/system/status", "/system/metrics", "/portfolio", "/risk"],
      },
      {
        href: "/markets",
        label: "Global market overview",
        mark: "MK",
        description: "Live market feed, instruments and the reference universe",
        reads: ["/stream/market", "/markets", "/assets", "/arbitrage"],
      },
      {
        href: "/command/regions",
        label: "Regional brain status",
        mark: "RB",
        description: "Every edge cell: health, book, staleness and its report age",
        reads: ["/regions", "/mesh"],
      },
      {
        href: "/system",
        label: "Platform health",
        mark: "PH",
        description: "Services, mesh, event-chain integrity and halt state",
        reads: ["/system", "/mesh", "/regions", "/stream/health"],
      },
      {
        href: "/command/alerts",
        label: "Alerts & incidents",
        mark: "AL",
        description: "Everything currently demanding attention, from real signals only",
        reads: ["/health", "/risk", "/orders", "/system/governance", "/regions"],
      },
    ],
  },
  {
    label: "Intelligence",
    items: [
      {
        href: "/signals",
        label: "Live opportunities",
        mark: "OP",
        description: "The opportunity queue, proposals, and the signal stream",
        reads: ["/opportunities", "/proposals", "/stream/signals"],
      },
      {
        href: "/loop",
        label: "The reasoning loop",
        mark: "LP",
        description: "What each of the eight stages produced, and refused, per cycle",
        reads: ["/cycle", "/system/metrics"],
      },
      {
        href: "/intelligence/predictions",
        label: "Market predictions",
        mark: "PR",
        description: "Model forecasts with confidence — illustrated until the surface exists",
        reads: [],
        simulated: true,
      },
      {
        href: "/intelligence/correlation",
        label: "Cross-market correlation",
        mark: "CX",
        description: "Pairwise co-movement across the universe — illustrated",
        reads: [],
        simulated: true,
      },
      {
        href: "/intelligence/news",
        label: "News & sentiment",
        mark: "NW",
        description: "What the platform ingests as narrative, and what it does not yet",
        reads: [],
        simulated: true,
      },
      {
        href: "/intelligence/regimes",
        label: "Regime detection",
        mark: "RG",
        description: "The market state the detectors currently believe — illustrated",
        reads: [],
        simulated: true,
      },
    ],
  },
  {
    label: "Strategies & Research",
    items: [
      {
        href: "/strategies",
        label: "Strategy library",
        mark: "ST",
        description: "Candidates, their rung, champion/challenger, and their capital",
        reads: ["/strategies", "/capital"],
      },
      {
        href: "/models",
        label: "Model registry",
        mark: "ML",
        description: "Model use, routing rungs, and the registry's own status",
        reads: ["/models", "/quantum", "/training"],
      },
      {
        href: "/research/backtesting",
        label: "Backtesting",
        mark: "BT",
        description: "The simulator that gates promotion, and where its results live",
        reads: ["/strategies"],
      },
      {
        href: "/research/quantum",
        label: "Quantum experiments",
        mark: "QX",
        description: "Routing, jobs, and the classical baseline computed every time",
        reads: ["/quantum", "/models"],
      },
    ],
  },
  {
    label: "Portfolio & Capital",
    items: [
      {
        href: "/portfolio",
        label: "Portfolio overview",
        mark: "PF",
        description: "The book's counts, paper-only state, and exposure",
        reads: ["/portfolio", "/capital", "/pnl", "/stream/positions"],
      },
      {
        href: "/capital",
        label: "Capital allocation",
        mark: "CP",
        description: "Bounds, envelopes issued, utilisation and outstanding recalls",
        reads: ["/capital", "/regions"],
      },
      {
        href: "/portfolio/positions",
        label: "Positions",
        mark: "PS",
        description: "Position-level detail, and exactly why it is gated today",
        reads: ["/portfolio", "/regions", "/stream/positions"],
      },
      {
        href: "/portfolio/pnl",
        label: "P&L & attribution",
        mark: "PL",
        description: "Profit, loss, and who or what earned it",
        reads: ["/pnl", "/portfolio"],
      },
    ],
  },
  {
    label: "Risk & Compliance",
    items: [
      {
        href: "/risk",
        label: "Global risk dashboard",
        mark: "RK",
        description: "Exposure, concentration, tail risk, limits and the kill switch",
        reads: ["/risk", "/capital", "/autonomy"],
      },
      {
        href: "/risk/limits",
        label: "Limit utilization",
        mark: "LU",
        description: "Every limit, how hard each is working, and which cannot fire",
        reads: ["/risk", "/capital"],
      },
      {
        href: "/risk/audit",
        label: "Audit trail",
        mark: "AU",
        description: "The hash-chained event log and its integrity, verified live",
        reads: ["/system", "/system/status"],
      },
    ],
  },
  {
    label: "Trading & Execution",
    items: [
      {
        href: "/orders",
        label: "Order blotter",
        mark: "OB",
        description: "Order lifecycle, refusals and reconciliation",
        reads: ["/orders", "/fills", "/stream/orders"],
      },
      {
        href: "/execution/fills",
        label: "Trades & fills",
        mark: "TF",
        description: "Every fill, its venue, and whether any was not simulated",
        reads: ["/fills", "/orders"],
      },
      {
        href: "/execution/venues",
        label: "Venue status",
        mark: "VN",
        description: "The venues the desk can reach, as the platform reports them",
        reads: ["/markets", "/assets", "/regions"],
      },
      {
        href: "/execution/arbitrage",
        label: "Multi-leg arbitrage",
        mark: "AR",
        description: "Active arbitrage paths and the engine's own status",
        reads: ["/arbitrage", "/opportunities"],
      },
    ],
  },
  {
    label: "Data & Operations",
    items: [
      {
        href: "/data-sources",
        label: "Feed catalog",
        mark: "DS",
        description: "Source registry, licensing posture, and the finder's status",
        reads: ["/data-sources", "/regions", "/mesh"],
      },
      {
        href: "/operations/mesh",
        label: "Data mesh",
        mark: "DM",
        description: "The backbone between cells and centre, counter by counter",
        reads: ["/mesh", "/regions"],
      },
      {
        href: "/operations/telemetry",
        label: "Telemetry & SLOs",
        mark: "TL",
        description: "The platform's own counters, watched over time by this console",
        reads: ["/system/metrics", "/system/status"],
      },
    ],
  },
  {
    label: "Administration",
    items: [
      {
        href: "/agents",
        label: "Agent roster",
        mark: "AG",
        description: "Every agent, its grants, and the governance review of the roster",
        reads: ["/agents", "/system/governance"],
      },
      {
        href: "/admin/autonomy",
        label: "Autonomy & governance",
        mark: "AN",
        description: "The autonomy level, its ceiling, and every change ever made",
        reads: ["/autonomy", "/system/governance"],
      },
      {
        href: "/admin/access",
        label: "Users & roles",
        mark: "UR",
        description: "The role each route requires, read from the live route table",
        reads: ["/openapi.json"],
      },
      {
        href: "/integrations",
        label: "API integrations",
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
