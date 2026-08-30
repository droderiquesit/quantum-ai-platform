/**
 * The Algorik brand: the shipped mark, the wordmark, and the rules.
 *
 * The assets are the real ones, supplied with the licensed brand package and
 * kept in `packages/brand/assets/`: the horizontal logo, the icon set, the
 * favicon, the Apple touch icon and the Android Chrome icons. Nothing here is
 * drawn from imagination — an earlier revision of this file carried an
 * invented "aperture" mark, written when the repository contained no Algorik
 * assets at all, and it is gone.
 *
 * The mark is an "A" inside two crossed orbital rings whose gradient runs
 * cyan through blue to violet over navy. Those are the colours
 * `@algorik/design-tokens` is derived from, which is why a surface that uses
 * the tokens already matches the logo without anyone matching them by eye.
 *
 * Components here render the raster assets rather than re-drawing them as SVG.
 * A hand-traced copy of a supplied logo is a second logo that drifts from the
 * first, and the brand guide names these files as the approved artwork.
 */
import type { CSSProperties } from "react";

/** Where the approved artwork lives, relative to a surface's public root. */
export const brandAssets = {
  /** Full horizontal lockup for headers and heroes, dark ink on transparent. */
  logo: "/brand/algorik-logo-transparent-1024w.png",
  /** The same lockup in white, for navy and photographic backgrounds. */
  logoWhite: "/brand/algorik-logo-white-1024w.png",
  /** Icon only, for tight spaces and app surfaces. */
  icon: "/brand/algorik-icon-master-transparent.png",
  favicon: "/brand/favicon.ico",
  appleTouchIcon: "/brand/apple-touch-icon.png",
  androidChrome192: "/brand/android-chrome-192x192.png",
  androidChrome512: "/brand/android-chrome-512x512.png",
} as const;

/**
 * Brand colours, as sampled from the shipped icon.
 *
 * Stated here so a surface that needs the literal brand hue — an OG image, an
 * email template, a `theme-color` meta tag — takes it from the same place the
 * design tokens were derived from, rather than from a screenshot.
 */
export const brandColors = {
  navy: "#071b4d",
  cyan: "#00c3fd",
  blue: "#005df8",
  violet: "#3700db",
} as const;

export interface MarkProps {
  readonly size?: number;
  /** Decorative beside the wordmark; labelled when it stands alone. */
  readonly title?: string;
  readonly className?: string;
  readonly style?: CSSProperties;
}

/**
 * The icon-only mark.
 *
 * `title` decides the accessibility role: given one it is an image with a
 * name, without one it is hidden. A logo that announces itself on every page
 * of a dense console is noise to a screen-reader user, and one that is silent
 * where it stands alone as a home link is a missing label.
 */
export function AlgorikMark({ size = 24, title, className, style }: MarkProps) {
  return (
    <img
      src={brandAssets.icon}
      width={size}
      height={size}
      alt={title ?? ""}
      aria-hidden={title ? undefined : true}
      className={className}
      style={{ display: "block", objectFit: "contain", ...style }}
      draggable={false}
    />
  );
}

export interface WordmarkProps extends MarkProps {
  /** Product qualifier, e.g. "Portal" or "Admin". Rendered lighter. */
  readonly qualifier?: string;
  /** Use the white lockup, for navy or photographic grounds. */
  readonly onDark?: boolean;
}

/**
 * The full lockup: the supplied horizontal logo, plus an optional qualifier.
 *
 * The qualifier is how a reader knows which Algorik surface they are on
 * without the surfaces needing to look different from one another. It is the
 * quiet half of the lockup on purpose — "Algorik" is the constant, and the
 * logo artwork itself is never altered to accommodate it.
 */
export function AlgorikWordmark({
  size = 24,
  qualifier,
  onDark = false,
  title = "Algorik",
  className,
  style,
}: WordmarkProps) {
  return (
    <span
      className={className}
      style={{ display: "inline-flex", alignItems: "center", gap: size * 0.42, ...style }}
    >
      <img
        src={onDark ? brandAssets.logoWhite : brandAssets.logo}
        alt={title}
        height={size}
        // The supplied lockup is close to 5:2. Width follows from height so the
        // artwork is never stretched to fit a box.
        width={Math.round(size * 2.5)}
        style={{ display: "block", height: size, width: "auto" }}
        draggable={false}
      />
      {qualifier ? (
        <span
          style={{
            fontSize: size * 0.44,
            fontWeight: 500,
            letterSpacing: "0.13em",
            textTransform: "uppercase",
            color: "var(--color-text-faint)",
            lineHeight: 1,
          }}
        >
          {qualifier}
        </span>
      ) : null}
    </span>
  );
}

/**
 * The product vocabulary, and the rule that one object has one name.
 *
 * The brief's example is the real failure: the same object called a signal on
 * one screen, a recommendation on the next and an opportunity on a third
 * teaches a user that the three are different things. They then ask which one
 * to act on. Every surface takes its noun from here.
 */
export const glossary = {
  opportunity:
    "A tradeable edge the platform has detected and scored. Never called a signal or a recommendation.",
  strategy: "A named, versioned decision procedure that can hold capital.",
  position: "A held quantity of an instrument, with its cost basis.",
  order: "An instruction to transact, in whatever state the venue has left it.",
  fill: "An executed portion of an order. On this platform, always simulated.",
  portfolio: "The whole book: positions, cash and their derived exposure.",
  capital: "Money allocated to a strategy or cell, bounded by an envelope.",
  envelope: "A time-bounded grant of capital a cell may trade inside, alone.",
  risk: "The measured exposure of the book and the limits bounding it.",
  limit: "A bound checked before an order exists. A limit that cannot fire is a defect.",
  killSwitch: "The control that halts trading. Halts are scoped and audited.",
  intelligence: "The platform's reasoning output: regimes, predictions, correlations.",
  cell: "A regional execution unit that decides alone within its envelope.",
  simulation: "Deterministic generated data, always labelled, never a measurement.",
  paperTrading:
    "Execution against a simulator. Algorik submits no live order; this is structural, not configuration.",
} as const;

export type GlossaryTerm = keyof typeof glossary;

/** The customer-facing sections, in order, shared by portal and mobile. */
export const productNavigation = [
  "Dashboard", "Portfolio", "Markets", "Opportunities", "Strategies",
  "Orders", "Positions", "Performance", "Capital", "Risk",
  "Intelligence", "Reports", "Integrations", "Account", "Security",
] as const;

/** The administrative sections. Deliberately a different vocabulary. */
export const adminNavigation = [
  "Operations", "Regions", "Data", "Models", "Strategies", "Execution",
  "Risk Controls", "Capital Controls", "Compliance", "Audit", "Incidents",
  "Deployments", "Platform Health", "Users and Organizations", "Configuration",
] as const;

/**
 * Claims this platform may not make, and what to say instead.
 *
 * Enforced by a test over landing copy rather than left to reviewer memory:
 * marketing text is written under deadline by people who did not read the
 * safety rules, and "guaranteed returns" on a trading site is a regulatory
 * event, not a wording preference.
 */
export const forbiddenClaims: readonly { readonly pattern: RegExp; readonly instead: string }[] = [
  { pattern: /guarantee\w*\s+(return|profit|alpha|gain|yield)/i, instead: "describe the method, never the outcome" },
  { pattern: /\bguaranteed\b/i, instead: "no outcome on this platform is guaranteed" },
  { pattern: /quantum\s+advantage/i, instead: "quantum methods run only against a measured classical baseline" },
  { pattern: /\b(sec|fca|finra)[- ]?(approved|registered|licensed)\b/i, instead: "state the regulatory position only when it is true and evidenced" },
  { pattern: /fully\s+autonomous\s+(live\s+)?trading/i, instead: "Algorik is paper trading; say so" },
  { pattern: /risk[- ]free/i, instead: "no trading is risk-free" },
  { pattern: /\b(outperform|beat)\s+the\s+market\b/i, instead: "describe the research, not a promise" },
] as const;

/** Returns every forbidden claim found in `copy`. Empty means the copy passes. */
export function auditCopy(copy: string): readonly { claim: string; instead: string }[] {
  return forbiddenClaims
    .map(({ pattern, instead }) => {
      const found = pattern.exec(copy);
      return found ? { claim: found[0], instead } : null;
    })
    .filter((entry): entry is { claim: string; instead: string } => entry !== null);
}
