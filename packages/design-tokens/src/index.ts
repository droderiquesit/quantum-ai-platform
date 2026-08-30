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
  canvas: "#07060e",
  surface: "#100f1d",
  surfaceElevated: "#171629",
  surfaceSunken: "#0b0a15",
  border: "#201f36",
  borderStrong: "#322f52",

  textPrimary: "#e7e6f4",
  textMuted: "#9a98b6",
  textFaint: "#636383",
  textOnBrand: "#0b0a15",

  brandPrimary: "#8f7bff",
  brandPrimaryMuted: "#282057",
  brandSecondary: "#35c8e8",
  accent: "#35c8e8",

  success: "#2ecc8f",
  warning: "#e3a72f",
  critical: "#ff5d6e",
  information: "#8f7bff",

  gain: "#2ecc8f",
  loss: "#ff5d6e",

  simulation: "#4c9aff",
  paper: "#b39bff",
  stage: "#e3a72f",
  live: "#ff3b30",

  focus: "#8f7bff",
  disabled: "#4a4a63",
};

/**
 * Light is the public site's default and the portal's option. It is not the
 * dark values lightened: contrast is recomputed against a white canvas, which
 * is why the brand darkens here rather than staying put.
 */
export const light: ColorScale = {
  canvas: "#f1f1f7",
  surface: "#ffffff",
  surfaceElevated: "#f6f6fb",
  surfaceSunken: "#e9e9f1",
  border: "#dedeea",
  borderStrong: "#c3c3d9",

  textPrimary: "#1b1a2e",
  textMuted: "#4e4f6e",
  textFaint: "#7f809b",
  textOnBrand: "#ffffff",

  brandPrimary: "#5d48d6",
  brandPrimaryMuted: "#e7e2fb",
  brandSecondary: "#0d7f9e",
  accent: "#0d7f9e",

  success: "#0c8f62",
  warning: "#9a6f0a",
  critical: "#d8394f",
  information: "#5d48d6",

  gain: "#0c8f62",
  loss: "#d8394f",

  simulation: "#1f6fd6",
  paper: "#6d48e0",
  stage: "#9a6f0a",
  live: "#cd2018",

  focus: "#5d48d6",
  disabled: "#9a9ab0",
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
