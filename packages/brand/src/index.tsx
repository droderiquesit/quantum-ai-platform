/**
 * The Algorik brand: one mark, one wordmark, one set of rules.
 *
 * Original work, drawn in this repository. Nothing here derives from a
 * purchased template or a third-party product's assets — `grep -rni algorik`
 * over the tree returned nothing when this programme began, so there was no
 * supplied brand to integrate and none has been borrowed since.
 *
 * The mark is an **aperture**: four arcs opening around a centre point. It
 * reads as a lens (the platform observes), as an iris (it opens by degrees —
 * simulation, paper, stage), and at 16px as a solid ring, which is the only
 * size test that matters for a favicon.
 *
 * Everything is `currentColor` or a token. The mark inherits the surface it
 * sits on, so it needs no light and dark variants and cannot go stale against
 * a theme change.
 */
import type { CSSProperties } from "react";

export interface MarkProps {
  readonly size?: number;
  /** Decorative beside the wordmark; labelled when it stands alone. */
  readonly title?: string;
  readonly className?: string;
  readonly style?: CSSProperties;
}

/**
 * The aperture mark.
 *
 * Four arcs at 90° with a deliberate gap: a closed ring is a hundred other
 * logos, and the gaps are what make it recognisable at a glance in a browser
 * tab. Arc weight is a fixed proportion of the box so it thins correctly when
 * scaled rather than turning into a blob at 16px.
 */
export function AlgorikMark({ size = 24, title, className, style }: MarkProps) {
  const stroke = size * 0.11;
  const radius = size * 0.34;
  const centre = size / 2;
  // 62° of arc per quadrant leaves 28° of gap — enough to read as separated
  // at 16px without the ring falling apart into four unrelated ticks.
  const sweep = 62;

  const arc = (startDegrees: number) => {
    const toPoint = (degrees: number) => {
      const radians = (degrees * Math.PI) / 180;
      return `${(centre + radius * Math.cos(radians)).toFixed(2)},${(
        centre + radius * Math.sin(radians)
      ).toFixed(2)}`;
    };
    return `M${toPoint(startDegrees)}A${radius},${radius} 0 0 1 ${toPoint(startDegrees + sweep)}`;
  };

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      className={className}
      style={style}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      focusable="false"
    >
      {[0, 90, 180, 270].map((start, index) => (
        <path
          key={start}
          d={arc(start + 14)}
          fill="none"
          stroke="currentColor"
          strokeWidth={stroke}
          strokeLinecap="round"
          // The two trailing arcs sit back so the mark reads as an opening
          // aperture rather than a static ring.
          opacity={index % 2 === 0 ? 1 : 0.55}
        />
      ))}
      <circle cx={centre} cy={centre} r={size * 0.085} fill="currentColor" />
    </svg>
  );
}

export interface WordmarkProps extends MarkProps {
  /** Product qualifier, e.g. "Portal" or "Admin". Rendered lighter. */
  readonly qualifier?: string;
  /** Hide the mark where one already appears nearby. */
  readonly markless?: boolean;
}

/**
 * The lockup: mark, name, and an optional surface qualifier.
 *
 * The qualifier is how a reader knows which Algorik surface they are on
 * without the surfaces having to look different from each other. It is the
 * quiet half of the lockup on purpose — "Algorik" is the constant.
 */
export function AlgorikWordmark({
  size = 24,
  qualifier,
  markless = false,
  title,
  className,
  style,
}: WordmarkProps) {
  return (
    <span
      className={className}
      style={{ display: "inline-flex", alignItems: "center", gap: size * 0.34, ...style }}
    >
      {markless ? null : <AlgorikMark size={size} title={title} />}
      <span style={{ display: "inline-flex", alignItems: "baseline", gap: size * 0.3 }}>
        <span
          style={{
            fontSize: size * 0.66,
            fontWeight: 600,
            letterSpacing: "0.02em",
            color: "var(--color-text-primary)",
            lineHeight: 1,
          }}
        >
          Algorik
        </span>
        {qualifier ? (
          <span
            style={{
              fontSize: size * 0.42,
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
