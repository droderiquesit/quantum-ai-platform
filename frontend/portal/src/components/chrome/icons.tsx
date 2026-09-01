/**
 * Inline Lucide icons (lucide.dev, ISC licence), as the SignalAIX template
 * uses them — inlined rather than loaded from a CDN so a deployment does not
 * depend on a third party being up to render its own navigation.
 *
 * Only the icons the shell actually uses live here. A page needing another
 * adds it here, named as upstream names it, so the set stays greppable
 * against the template's data-lucide attributes.
 */
import type { SVGProps } from "react";

const P: Record<string, string[]> = {
  "layout-dashboard": ["M3 3h7v9H3z", "M14 3h7v5h-7z", "M14 12h7v9h-7z", "M3 16h7v5H3z"],
  radio: ["M4.9 19.1C1 15.2 1 8.8 4.9 4.9", "M7.8 16.2c-2.3-2.3-2.3-6.1 0-8.5", "M16.2 7.8c2.3 2.3 2.3 6.1 0 8.5", "M19.1 4.9C23 8.8 23 15.2 19.1 19.1", "M12 12h.01"],
  sparkles: ["M9.9 3.9 12 9l5.1 2.1L12 13.2 9.9 18.3 7.8 13.2 2.7 11.1 7.8 9z", "M18 2l1 3 3 1-3 1-1 3-1-3-3-1 3-1z"],
  "line-chart": ["M3 3v18h18", "m19 9-5 5-4-4-3 3"],
  "pie-chart": ["M21.21 15.89A10 10 0 1 1 8 2.83", "M22 12A10 10 0 0 0 12 2v10z"],
  "shield-check": ["M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1 1 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z", "m9 12 2 2 4-4"],
  wallet: ["M19 7V4a1 1 0 0 0-1-1H5a2 2 0 0 0 0 4h15a1 1 0 0 1 1 1v4h-3a2 2 0 0 0 0 4h3a1 1 0 0 0 1-1v-2a1 1 0 0 0-1-1", "M3 5v14a2 2 0 0 0 2 2h15a1 1 0 0 0 1-1v-4"],
  "table-2": ["M9 3H5a2 2 0 0 0-2 2v4m6-6h10a2 2 0 0 1 2 2v4M9 3v18m0 0h10a2 2 0 0 0 2-2V9M9 21H5a2 2 0 0 1-2-2V9m0 0h18"],
  waypoints: ["M12 7a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z", "M19.5 22a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z", "M4.5 22a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z", "M12 7v4", "m6 17.5 4.5-4.5", "m13.5 13 4.5 4.5"],
  "user-cog": ["M10 15a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z", "M2 21a8 8 0 0 1 10.4-7.6", "M18 22a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z", "m19.5 14.3-.4.9", "m16.9 20.8-.4.9", "m21.7 19.5-.9-.4", "m15.2 16.9-.9-.4", "m21.7 16.5-.9.4", "m15.2 19.1-.9.4", "m19.5 21.7-.4-.9", "m16.9 15.2-.4-.9"],
  menu: ["M4 6h16", "M4 12h16", "M4 18h16"],
  "panel-left": ["M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z", "M9 3v18"],
  search: ["m21 21-4.34-4.34", "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16Z"],
  bell: ["M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9", "M10.3 21a1.94 1.94 0 0 0 3.4 0"],
  moon: ["M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"],
  sun: ["M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10Z", "M12 1v2", "M12 21v2", "M4.22 4.22l1.42 1.42", "M18.36 18.36l1.42 1.42", "M1 12h2", "M21 12h2", "M4.22 19.78l1.42-1.42", "M18.36 5.64l1.42-1.42"],
  x: ["M18 6 6 18", "m6 6 12 12"],
  "alert-triangle": ["m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z", "M12 9v4", "M12 17h.01"],
  "log-out": ["M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4", "m16 17 5-5-5-5", "M21 12H9"],
  "help-circle": ["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Z", "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3", "M12 17h.01"],
  activity: ["M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"],
  "scan-line": ["M3 7V5a2 2 0 0 1 2-2h2", "M17 3h2a2 2 0 0 1 2 2v2", "M21 17v2a2 2 0 0 1-2 2h-2", "M7 21H5a2 2 0 0 1-2-2v-2", "M7 12h10"],
  plug: ["M12 22v-5", "M9 8V2", "M15 8V2", "M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z"],
};

export type IconName = keyof typeof P & string;

export function Icon({
  name,
  className = "w-5 h-5 shrink-0",
  ...rest
}: { name: IconName } & SVGProps<SVGSVGElement>) {
  const paths = P[name] ?? P["layout-dashboard"];
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      className={className}
      {...rest}
    >
      {paths?.map((d) => <path key={d} d={d} />)}
    </svg>
  );
}

/** The template's per-section icons, mapped onto Algorik's navigation. */
export const SECTION_ICON: Record<string, IconName> = {
  "Command Center": "layout-dashboard",
  Intelligence: "sparkles",
  "Strategies & Research": "line-chart",
  "Portfolio & Capital": "wallet",
  "Risk & Compliance": "shield-check",
  "Trading & Execution": "activity",
  "Data & Operations": "waypoints",
  Administration: "user-cog",
};

export const ITEM_ICON: Record<string, IconName> = {
  "/": "layout-dashboard",
  "/signals": "radio",
  "/loop": "waypoints",
  "/markets": "line-chart",
  "/command/regions": "scan-line",
  "/command/alerts": "alert-triangle",
  "/system": "activity",
  "/intelligence/predictions": "sparkles",
  "/intelligence/correlation": "table-2",
  "/intelligence/news": "radio",
  "/intelligence/regimes": "pie-chart",
  "/strategies": "line-chart",
  "/models": "sparkles",
  "/research/backtesting": "activity",
  "/research/quantum": "sparkles",
  "/portfolio": "wallet",
  "/capital": "pie-chart",
  "/portfolio/positions": "table-2",
  "/portfolio/pnl": "line-chart",
  "/risk": "shield-check",
  "/risk/limits": "shield-check",
  "/risk/audit": "scan-line",
  "/orders": "table-2",
  "/execution/fills": "activity",
  "/execution/venues": "plug",
  "/execution/arbitrage": "waypoints",
  "/data-sources": "plug",
  "/operations/mesh": "waypoints",
  "/operations/telemetry": "activity",
  "/agents": "user-cog",
  "/admin/autonomy": "shield-check",
  "/admin/access": "user-cog",
  "/integrations": "plug",
};
