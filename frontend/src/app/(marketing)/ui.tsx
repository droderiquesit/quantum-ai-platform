/**
 * Shared building blocks for the public marketing pages.
 *
 * Same design system as the portal — every colour is a token from
 * `@algorik/design-tokens` via the custom properties in `globals.css` — worn
 * more spaciously: one measured column, 8–14px radii, generous section
 * padding, editorial type. A marketing page that invented its own palette
 * would be the first step toward the public site and the product looking
 * like two different companies.
 *
 * Everything here is a server component. The only client code in the
 * marketing group is `AuthCta`, which needs the browser to ask about the
 * session.
 */
import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";

/** Every marketing section shares one measured column. */
export function Column({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={`mx-auto w-full max-w-[1080px] px-6 ${className ?? ""}`}>{children}</div>;
}

/**
 * A page's opening: eyebrow, display headline, lede, optional actions.
 * The one h1 a page gets; sections below it use h2.
 */
export function PageIntro({
  eyebrow,
  title,
  lede,
  children,
}: {
  eyebrow: string;
  title: string;
  lede: string;
  children?: ReactNode;
}) {
  return (
    <section className="border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)]">
      <Column className="pb-16 pt-14">
        <p className="eyebrow">{eyebrow}</p>
        <h1 className="mt-3 max-w-[860px] text-[34px] font-semibold leading-[1.08] tracking-[-0.02em] sm:text-[44px]">
          {title}
        </h1>
        <p className="mt-5 max-w-[700px] text-[15px] leading-[1.65] text-[color:var(--color-ink-dim)]">
          {lede}
        </p>
        {children}
      </Column>
    </section>
  );
}

/** A titled band of content. `raised` alternates the background for rhythm. */
export function Section({
  id,
  eyebrow,
  title,
  lede,
  raised = false,
  children,
}: {
  id?: string;
  eyebrow?: string;
  title: string;
  lede?: string;
  raised?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      id={id}
      className={
        raised
          ? "border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)]"
          : "border-b border-[color:var(--color-line)]"
      }
    >
      <Column className="py-16">
        {eyebrow ? <p className="eyebrow">{eyebrow}</p> : null}
        <h2 className="mt-2 max-w-[760px] text-[26px] font-semibold leading-[1.2] tracking-[-0.01em]">
          {title}
        </h2>
        {lede ? (
          <p className="mt-4 max-w-[700px] text-[14.5px] leading-[1.65] text-[color:var(--color-ink-dim)]">
            {lede}
          </p>
        ) : null}
        <div className="mt-8">{children}</div>
      </Column>
    </section>
  );
}

/**
 * The `.btn` control at editorial size. Inline style rather than utility
 * classes because the console stylesheet's `.btn` is unlayered and outranks
 * Tailwind's layered utilities — and an inline size is legible about being a
 * deliberate marketing exception rather than a fight with the cascade.
 */
const CTA_SIZE: CSSProperties = { height: "40px", paddingInline: "20px", fontSize: "12px" };

export function CtaLink({
  href,
  variant,
  children,
}: {
  href: string;
  variant?: "primary" | "ghost";
  children: ReactNode;
}) {
  return (
    <Link href={href} className="btn" data-variant={variant} style={CTA_SIZE}>
      {children}
    </Link>
  );
}

export function CardGrid({ children, columns = 3 }: { children: ReactNode; columns?: 2 | 3 }) {
  return (
    <div className={columns === 3 ? "grid gap-4 md:grid-cols-3" : "grid gap-4 md:grid-cols-2"}>
      {children}
    </div>
  );
}

/**
 * A feature card. `meta` is for the honesty chip — "served today" or
 * "in research" — so a capability's real status is on the card itself
 * rather than in a footnote nobody reads.
 */
export function Card({
  title,
  meta,
  children,
}: {
  title: string;
  meta?: string;
  children: ReactNode;
}) {
  return (
    <div className="rounded-[14px] border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-6">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-2">
        <h3 className="text-[15px] font-semibold">{title}</h3>
        {meta ? <span className="chip">{meta}</span> : null}
      </div>
      <div className="mt-3 space-y-3 text-[13.5px] leading-[1.65] text-[color:var(--color-ink-dim)]">
        {children}
      </div>
    </div>
  );
}

/** The closing call to action every page ends on. */
export function CtaBand({
  title,
  lede,
  primaryHref = "/sign-up",
  primaryLabel = "Get started",
  secondaryHref = "/platform",
  secondaryLabel = "See the platform",
}: {
  title: string;
  lede: string;
  primaryHref?: string;
  primaryLabel?: string;
  secondaryHref?: string;
  secondaryLabel?: string;
}) {
  return (
    <section>
      <Column className="py-16">
        <div className="rounded-[14px] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-8 py-12 text-center">
          <h2 className="text-[26px] font-semibold leading-[1.2] tracking-[-0.01em]">{title}</h2>
          <p className="mx-auto mt-4 max-w-[560px] text-[14px] leading-[1.65] text-[color:var(--color-ink-dim)]">
            {lede}
          </p>
          <div className="mt-7 flex flex-wrap items-center justify-center gap-3">
            <CtaLink href={primaryHref} variant="primary">
              {primaryLabel}
            </CtaLink>
            <CtaLink href={secondaryHref}>{secondaryLabel}</CtaLink>
          </div>
        </div>
      </Column>
    </section>
  );
}

/** A bulleted list in the marketing register. */
export function Bullets({ items }: { items: readonly ReactNode[] }) {
  return (
    <ul className="list-disc space-y-2 pl-5 text-[13.5px] leading-[1.65] text-[color:var(--color-ink-dim)]">
      {items.map((item, index) => (
        <li key={index}>{item}</li>
      ))}
    </ul>
  );
}

/**
 * The notice every legal draft opens with. Bordered in the warning token so
 * it cannot be mistaken for boilerplate: none of these documents is in
 * force, and a page that read as an effective policy would be a false
 * statement about the company.
 */
export function DraftNotice() {
  return (
    <p
      role="note"
      className="rounded-[8px] border border-[color:var(--color-warn)] bg-[color:var(--color-surface)] px-5 py-4 text-[13px] leading-[1.6]"
    >
      <strong className="font-semibold">Draft for review</strong> — not yet reviewed by counsel.
      Not effective until published with an effective date.
    </p>
  );
}

/** The frame every legal draft shares: eyebrow, title, draft notice, body. */
export function LegalShell({ title, children }: { title: string; children: ReactNode }) {
  return (
    <article className="border-b border-[color:var(--color-line)]">
      <Column className="py-16">
        <div className="max-w-[760px]">
          <p className="eyebrow">Legal</p>
          <h1 className="mt-3 text-[34px] font-semibold leading-[1.1] tracking-[-0.02em]">
            {title}
          </h1>
          <div className="mt-6">
            <DraftNotice />
          </div>
          <div className="mt-10 space-y-10">{children}</div>
        </div>
      </Column>
    </article>
  );
}

export function LegalSection({ heading, children }: { heading: string; children: ReactNode }) {
  return (
    <section>
      <h2 className="text-[19px] font-semibold">{heading}</h2>
      <div className="mt-3 space-y-3 text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
        {children}
      </div>
    </section>
  );
}
