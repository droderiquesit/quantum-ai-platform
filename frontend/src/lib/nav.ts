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
 * The console's map.
 *
 * Grouped the way a desk is organised rather than the way the API is: what is
 * happening, what we hold, what could stop us, and what is running.
 */
export const NAV: readonly NavGroup[] = [
  {
    label: "Desk",
    items: [
      {
        href: "/",
        label: "Executive overview",
        mark: "EX",
        description: "Halt state, autonomy, book counts and the loop's cadence",
        reads: ["/health", "/system/status", "/system/metrics", "/portfolio", "/risk"],
      },
    ],
  },
  {
    label: "Market",
    items: [
      {
        href: "/markets",
        label: "Global markets",
        mark: "MK",
        description: "Live market and signal feeds",
        reads: ["/stream/market", "/stream/signals", "/markets", "/assets"],
      },
      {
        href: "/data-sources",
        label: "Data sources",
        mark: "DS",
        description: "Source registry, latency, freshness, quality and provenance",
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
        description: "Positions, cash, exposures and attribution",
        reads: ["/portfolio", "/capital", "/pnl", "/stream/positions"],
      },
      {
        href: "/orders",
        label: "Order blotter",
        mark: "OB",
        description: "Live order lifecycle and fill history",
        reads: ["/orders", "/fills", "/stream/orders"],
      },
      {
        href: "/order-entry",
        label: "Paper order entry",
        mark: "OE",
        description: "Stage and submit a simulated order",
        reads: ["/system/status", "/risk"],
      },
    ],
  },
  {
    label: "Control",
    items: [
      {
        href: "/risk",
        label: "Risk & compliance",
        mark: "RK",
        description: "Exposure, concentration, governance and the kill switch",
        reads: ["/risk", "/autonomy", "/system/governance", "/capital"],
      },
      {
        href: "/system",
        label: "System topology",
        mark: "SY",
        description: "Services, mesh, edge cells, agents and event-chain integrity",
        reads: ["/system", "/mesh", "/regions", "/agents", "/stream/health"],
      },
    ],
  },
];

export const NAV_ITEMS: readonly NavItem[] = NAV.flatMap((group) => group.items);

export function navItemFor(pathname: string): NavItem | undefined {
  return NAV_ITEMS.find((item) => item.href === pathname);
}
