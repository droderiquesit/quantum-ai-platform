/**
 * Algorik design tokens — the single source of truth for every surface.
 *
 * Landing, portal, admin and the installed mobile app all derive from this
 * file. A colour, a size or a duration that appears anywhere else is a defect:
 * it is the mechanism by which a landing page starts looking like one company
 * and a portal like a purchased template.
 *
 * Two rules the values themselves encode:
 *
 * **Semantic, not literal.** Nothing here is named for what it looks like.
 * `surface` and `critical` survive a rebrand; `grey900` and `red500` do not,
 * and a token named for its hue is one somebody will eventually use for the
 * wrong meaning because the hue happened to fit.
 *
 * **Financial state never rests on hue alone.** `gain` and `loss` carry a
 * paired glyph and label in `financialState` below, because roughly one man in
 * twelve cannot separate them by colour, and a trading surface that encodes
 * profit and loss in red and green alone is unreadable to him.
 *
 * Consumed two ways: as CSS custom properties emitted by `cssVariables()`, and
 * as plain values by any platform that has no CSS — which is why this file
 * imports nothing and knows nothing about the DOM.
 *
 * **Where these values come from.** Per ADR 0015 the licensed SignalAIX
 * template is the visual source of truth, so both themes carry its palette
 * verbatim (`vendor/templates/signalaix/source-code/src/css/common.css`): emerald accent,
 * indigo secondary, slate light theme, near-black dark. The Algorik logo
 * (navy/cyan/blue/violet) remains the brand *mark* and reads correctly on
 * these surfaces; the interface colour system is the template's own, because
 * "the same as what was purchased" is the requirement and a hand-matched
 * approximation is the thing that drifts.
 */

/** A theme's complete semantic colour set. Both themes define every key. */
export interface ColorScale {
  /** The page behind everything. */
  readonly canvas: string;
  /** A panel or card sitting on the canvas. */
  readonly surface: string;
  /** A surface raised above another surface. */
  readonly surfaceElevated: string;
  /** A surface pressed into the page — table heads, wells. */
  readonly surfaceSunken: string;
  readonly border: string;
  readonly borderStrong: string;

  readonly textPrimary: string;
  readonly textMuted: string;
  readonly textFaint: string;
  /** Text on top of a brand-primary fill. */
  readonly textOnBrand: string;

  readonly brandPrimary: string;
  readonly brandPrimaryMuted: string;
  readonly brandSecondary: string;
  readonly accent: string;

  readonly success: string;
  readonly warning: string;
  readonly critical: string;
  readonly information: string;

  /** Financial direction. Never the only signal — see `financialState`. */
  readonly gain: string;
  readonly loss: string;

  /** Deployment posture. `live` is an alarm, not a mode. See ADR 0014. */
  readonly simulation: string;
  readonly paper: string;
  readonly stage: string;
  readonly live: string;

  readonly focus: string;
  readonly disabled: string;
}

/**
 * Dark is Algorik's primary theme: the portal is read for hours in rooms kept
 * dim so screens are legible, and the brand was drawn for it.
 */
export const dark: ColorScale = {
  canvas: "#050709",
  surface: "#0a0d12",
  surfaceElevated: "#10141c",
  surfaceSunken: "#030507",
  border: "#1a1f2e",
  borderStrong: "#252c3f",

  textPrimary: "#f1f5f9",
  textMuted: "#94a3b8",
  textFaint: "#64748b",
  textOnBrand: "#052e22",

  brandPrimary: "#10b981",
  brandPrimaryMuted: "#0a2f25",
  brandSecondary: "#6366f1",
  accent: "#6366f1",

  success: "#10b981",
  warning: "#f59e0b",
  critical: "#ef4444",
  information: "#0ea5e9",

  gain: "#10b981",
  loss: "#ef4444",

  simulation: "#0ea5e9",
  paper: "#6366f1",
  stage: "#f59e0b",
  live: "#ef4444",

  focus: "#10b981",
  disabled: "#475569",
};

/**
 * Light is the public site's default and the portal's option. It is not the
 * dark values lightened: contrast is recomputed against a white canvas, which
 * is why the brand darkens here rather than staying put.
 */
export const light: ColorScale = {
  canvas: "#f8fafc",
  surface: "#ffffff",
  surfaceElevated: "#f1f5f9",
  surfaceSunken: "#eef2f7",
  border: "#e2e8f0",
  borderStrong: "#cbd5e1",

  textPrimary: "#0f172a",
  textMuted: "#64748b",
  textFaint: "#94a3b8",
  textOnBrand: "#ffffff",

  brandPrimary: "#059669",
  brandPrimaryMuted: "#d1fae5",
  brandSecondary: "#6366f1",
  accent: "#6366f1",

  success: "#059669",
  warning: "#d97706",
  critical: "#dc2626",
  information: "#0284c7",

  gain: "#059669",
  loss: "#dc2626",

  simulation: "#0284c7",
  paper: "#6366f1",
  stage: "#d97706",
  live: "#dc2626",

  focus: "#059669",
  disabled: "#94a3b8",
};

/**
 * Direction, said three ways.
 *
 * Every financial value renders its colour, its glyph and — for assistive
 * technology — its word. Colour alone fails for colour-blind readers, in
 * greyscale print, and in a screenshot pasted into a document.
 */
export const financialState = {
  gain: { token: "gain", glyph: "▲", label: "up" },
  loss: { token: "loss", glyph: "▼", label: "down" },
  flat: { token: "textMuted", glyph: "—", label: "unchanged" },
} as const;

export type FinancialDirection = keyof typeof financialState;

/**
 * Type scale. `financialNumeric` and `table` are monospaced with tabular
 * figures so digits align in a column — a price column that does not align
 * cannot be scanned, which is the whole reason a trader looks at one.
 */
export const typography = {
  display: { size: "44px", lineHeight: "1.08", weight: 600, tracking: "-0.02em" },
  pageTitle: { size: "26px", lineHeight: "1.2", weight: 600, tracking: "-0.01em" },
  sectionTitle: { size: "17px", lineHeight: "1.3", weight: 600, tracking: "0" },
  cardTitle: { size: "13px", lineHeight: "1.35", weight: 600, tracking: "0.02em" },
  body: { size: "14px", lineHeight: "1.55", weight: 400, tracking: "0" },
  bodySmall: { size: "12.5px", lineHeight: "1.5", weight: 400, tracking: "0" },
  label: { size: "10px", lineHeight: "1.2", weight: 600, tracking: "0.09em" },
  financialNumeric: { size: "12px", lineHeight: "1.3", weight: 500, tracking: "-0.01em" },
  table: { size: "12px", lineHeight: "1.4", weight: 400, tracking: "0" },
  caption: { size: "11px", lineHeight: "1.45", weight: 400, tracking: "0" },
  code: { size: "11.5px", lineHeight: "1.5", weight: 400, tracking: "0" },
} as const;

export const fontFamily = {
  sans: '"Inter", "SF Pro Text", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
  mono: 'ui-monospace, "SF Mono", "SFMono-Regular", "JetBrains Mono", Menlo, Consolas, "Liberation Mono", monospace',
} as const;

/** A 4px base. Every gap on every surface is one of these. */
export const space = {
  "0": "0px", "1": "4px", "2": "8px", "3": "12px", "4": "16px",
  "5": "20px", "6": "24px", "8": "32px", "10": "40px", "12": "48px",
  "16": "64px", "20": "80px", "24": "96px",
} as const;

/**
 * Radii. The portal is nearly square because density reads better with hard
 * corners; the public site uses `lg` and `xl`, which is most of why the two
 * feel different while sharing everything else.
 */
export const radius = {
  none: "0px", sm: "2px", md: "4px", lg: "8px", xl: "14px", pill: "999px",
} as const;

/** Elevation. Dark themes separate by border first, shadow second. */
export const elevation = {
  none: "none",
  sm: "0 1px 2px rgba(0,0,0,0.28)",
  md: "0 4px 12px rgba(0,0,0,0.32)",
  lg: "0 12px 32px rgba(0,0,0,0.38)",
} as const;

/**
 * Motion. Every duration here is under a fifth of a second: on a surface
 * where numbers change while you read them, animation that outlasts a glance
 * is animation that hides the change it is meant to draw attention to.
 *
 * Every consumer must honour `prefers-reduced-motion`.
 */
export const motion = {
  instant: "0ms",
  fast: "90ms",
  base: "150ms",
  slow: "190ms",
  easeOut: "cubic-bezier(0.16, 1, 0.3, 1)",
  easeInOut: "cubic-bezier(0.65, 0, 0.35, 1)",
} as const;

/** The breakpoints every surface is tested at, smallest first. */
export const breakpoints = {
  xs: 320, sm: 375, md: 768, lg: 1024, xl: 1440, xxl: 1920,
} as const;

export const zIndex = {
  base: 0, sticky: 10, drawer: 60, banner: 70, dialog: 80, toast: 90, palette: 100,
} as const;

/** Kebab-cases a camelCase token name for its CSS custom property. */
function kebab(name: string): string {
  return name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

/**
 * The theme as CSS custom properties.
 *
 * Emitting rather than hand-maintaining the CSS is what keeps this file the
 * only place a colour is decided. A stylesheet that redeclared them would
 * drift from here on the first hurried change, and drift silently.
 */
export function cssVariables(scale: ColorScale): string {
  return Object.entries(scale)
    .map(([name, value]) => `  --color-${kebab(name)}: ${value};`)
    .join("\n");
}

export const tokens = {
  color: { dark, light },
  financialState, typography, fontFamily, space, radius, elevation, motion,
  breakpoints, zIndex,
} as const;
