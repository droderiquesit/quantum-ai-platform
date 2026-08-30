import Link from "next/link";
import type { ReactNode } from "react";
import { AlgorikWordmark } from "@algorik/brand";
import { AuthCta } from "./AuthCta";
import { Column } from "./ui";

/**
 * The public marketing group: header, footer, and the light default.
 *
 * Route groups do not appear in URLs, so these pages serve /welcome,
 * /platform and so on while the portal keeps the console shell to itself.
 * The chrome here renders no platform data at all — the one dynamic element
 * is `AuthCta`, whose session answer deliberately carries nothing a public
 * page may not show.
 */

const NAV = [
  { label: "Platform", href: "/platform" },
  { label: "Technology", href: "/technology" },
  { label: "Security", href: "/security" },
  { label: "Institutional", href: "/institutional" },
  { label: "Developers", href: "/developers" },
  { label: "Company", href: "/company" },
] as const;

/**
 * Light is the marketing default; dark is the portal's. The root layout's
 * boot script applies a stored choice, so this one acts only when there is
 * none: a visitor who explicitly chose a theme anywhere on the site keeps it
 * here too. Inline and blocking so the default is applied before these pages
 * paint, and try/catch because storage can be denied.
 */
const LIGHT_DEFAULT = `try{if(localStorage.getItem("algorik.theme")===null)document.documentElement.dataset.theme="light"}catch(e){}`;

const FOOTER_COLUMNS = [
  {
    title: "Platform",
    links: [
      { label: "Platform", href: "/platform" },
      { label: "Technology", href: "/technology" },
      { label: "Developers", href: "/developers" },
    ],
  },
  {
    title: "Company",
    links: [
      { label: "About", href: "/company" },
      { label: "Institutional", href: "/institutional" },
      { label: "Contact", href: "/contact" },
    ],
  },
  {
    title: "Legal",
    links: [
      { label: "Terms of service", href: "/legal/terms" },
      { label: "Privacy policy", href: "/legal/privacy" },
      { label: "Risk disclosures", href: "/legal/risk-disclosures" },
    ],
  },
  {
    title: "Security",
    links: [
      { label: "Security overview", href: "/security" },
      { label: "Paper-trading boundary", href: "/security#boundary" },
      { label: "Audit log", href: "/security#audit" },
    ],
  },
] as const;

export default function MarketingLayout({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col bg-[color:var(--color-void)] text-[color:var(--color-ink)]">
      <script dangerouslySetInnerHTML={{ __html: LIGHT_DEFAULT }} />
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-[100] focus:border focus:border-[color:var(--color-accent)] focus:bg-[color:var(--color-surface)] focus:px-3 focus:py-1.5 focus:text-[12px]"
      >
        Skip to content
      </a>
      <Header />
      <main id="main-content" className="flex-1">
        {children}
      </main>
      <Footer />
    </div>
  );
}

function Header() {
  return (
    <header className="sticky top-0 z-10 border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)]">
      <Column className="flex h-[60px] items-center gap-4">
        <Link href="/welcome" aria-label="Algorik home" className="shrink-0">
          <AlgorikWordmark size={24} />
        </Link>
        {/* Posture is declared wherever it could be wondered about. The chip
            uppercases, so this reads PAPER TRADING. */}
        <span className="chip hidden sm:inline-flex" data-tone="info">
          Paper trading
        </span>
        <nav aria-label="Primary" className="ml-2 hidden items-center gap-1 md:flex">
          {NAV.map((item) => (
            <Link
              key={item.href}
              href={item.href}
              className="rounded-[8px] px-3 py-2 text-[13px] text-[color:var(--color-ink-dim)] hover:bg-[color:var(--color-raised)] hover:text-[color:var(--color-ink)]"
            >
              {item.label}
            </Link>
          ))}
        </nav>
        <div className="ml-auto flex shrink-0 items-center gap-2">
          <AuthCta />
          <MobileMenu />
        </div>
      </Column>
    </header>
  );
}

/**
 * Below md the nav collapses into a native disclosure. <details> because a
 * menu that needs JavaScript is a menu that fails exactly when the page is
 * slowest, and the native element is keyboard- and screen-reader-accessible
 * for free.
 */
function MobileMenu() {
  return (
    <details className="relative md:hidden">
      <summary
        className="btn list-none [&::-webkit-details-marker]:hidden"
        aria-label="Site navigation"
      >
        Menu
      </summary>
      <nav
        aria-label="Primary"
        className="absolute right-0 top-[34px] flex w-56 flex-col rounded-[8px] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] p-2 shadow-lg"
      >
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            className="rounded-[4px] px-3 py-2 text-[13px] text-[color:var(--color-ink-dim)] hover:bg-[color:var(--color-raised)] hover:text-[color:var(--color-ink)]"
          >
            {item.label}
          </Link>
        ))}
      </nav>
    </details>
  );
}

function Footer() {
  return (
    <footer className="border-t border-[color:var(--color-line)] bg-[color:var(--color-surface)]">
      <Column className="py-12">
        <div className="grid gap-10 md:grid-cols-[1.4fr_1fr_1fr_1fr_1fr]">
          <div>
            <AlgorikWordmark size={20} />
            <p className="mt-4 max-w-[280px] text-[12.5px] leading-[1.6] text-[color:var(--color-ink-dim)]">
              Research-grade algorithmic intelligence, with the audit trail to show exactly what it
              did and why.
            </p>
          </div>
          {FOOTER_COLUMNS.map((column) => (
            <nav key={column.title} aria-label={`Footer: ${column.title}`}>
              <h2 className="eyebrow">{column.title}</h2>
              <ul className="mt-3 space-y-2">
                {column.links.map((link) => (
                  <li key={link.href}>
                    <Link
                      href={link.href}
                      className="text-[12.5px] text-[color:var(--color-ink-dim)] hover:text-[color:var(--color-ink)]"
                    >
                      {link.label}
                    </Link>
                  </li>
                ))}
              </ul>
            </nav>
          ))}
        </div>
        <p className="mt-10 border-t border-[color:var(--color-line)] pt-6 text-[12px] leading-[1.6] text-[color:var(--color-ink-faint)]">
          Algorik is a paper-trading research platform. Simulated execution only — no live orders
          are submitted.
        </p>
      </Column>
    </footer>
  );
}
