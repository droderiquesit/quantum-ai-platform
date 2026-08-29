import type { ReactNode } from "react";

/**
 * The labels that keep a simulated page honest.
 *
 * Everything rendered from `src/lib/sim` sits under one of these. The banner
 * states three things a reader needs before trusting anything below it: that
 * the figures are generated, that they are deterministic rather than live,
 * and which platform surface would replace them. A simulated figure that
 * could be screenshotted without its label is a fabrication with extra steps.
 */
export function SimulatedBanner({
  subject,
  contract,
  children,
}: {
  /** What is being illustrated, e.g. "market predictions". */
  subject: string;
  /** The endpoint that would make this page real, e.g. "GET /api/v1/predictions". */
  contract?: string;
  children?: ReactNode;
}) {
  return (
    <div
      role="note"
      data-testid="simulated-banner"
      className="flex flex-col gap-1.5 border border-dashed border-[color:var(--color-quantum)]/50 bg-[color:var(--color-surface)] px-3 py-2.5"
    >
      <div className="flex flex-wrap items-center gap-2">
        <SimChip />
        <span className="text-[12.5px] font-medium text-[color:var(--color-ink)]">
          Every figure below is generated, not measured.
        </span>
      </div>
      <p className="max-w-[80ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
        The platform serves no {subject} surface yet, so this page renders a deterministic
        illustration from a fixed seed — identical on every load, on every machine, moving never.
        {contract ? (
          <>
            {" "}
            It is written against the contract <code className="num">{contract}</code>; when the
            platform serves it, the adapter swaps and the page keeps working.
          </>
        ) : null}
      </p>
      {children}
    </div>
  );
}

/** The inline mark for a single simulated figure inside a mixed panel. */
export function SimChip() {
  return (
    <span
      className="chip"
      style={{
        color: "var(--color-quantum)",
        borderColor: "color-mix(in srgb, var(--color-quantum) 55%, transparent)",
      }}
    >
      simulated data
    </span>
  );
}
