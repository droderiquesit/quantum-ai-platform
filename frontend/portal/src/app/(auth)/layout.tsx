import Link from "next/link";
import { AlgorikWordmark } from "@algorik/brand";

/**
 * The auth group wears almost nothing, on purpose. A person signing in has one
 * job, and console chrome around a password field is both a distraction and a
 * lie — none of the navigation it offers would work without the session the
 * form is trying to create. So: the wordmark as the way back out, the card,
 * and the paper-trading declaration.
 *
 * The declaration is in a fixed footer rather than inside the card so it is
 * present on every page of the group without any page having to remember it —
 * including the error pages, which are exactly where a screenshot is most
 * likely to be taken.
 */
export default function AuthLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col items-center justify-center gap-5 bg-[color:var(--color-canvas)] px-4 pb-24 pt-10">
      <Link href="/welcome">
        <AlgorikWordmark size={26} onDark />
      </Link>
      <main className="w-full max-w-[400px] border border-[color:var(--color-border)] bg-[color:var(--color-surface)] p-5">
        {children}
      </main>
      <footer className="fixed inset-x-0 bottom-0 border-t border-[color:var(--color-border)] bg-[color:var(--color-canvas)] px-4 py-2 text-center text-[11px] text-[color:var(--color-ink-faint)]">
        <p>Paper trading only — no control on this platform submits a live order.</p>
        <nav className="mt-1 flex justify-center gap-4">
          <Link href="/legal/terms" className="hover:text-[color:var(--color-ink)]">
            Terms of Service
          </Link>
          <Link href="/legal/privacy" className="hover:text-[color:var(--color-ink)]">
            Privacy Policy
          </Link>
          <Link href="/legal/risk-disclosures" className="hover:text-[color:var(--color-ink)]">
            Risk Disclosures
          </Link>
        </nav>
      </footer>
    </div>
  );
}
