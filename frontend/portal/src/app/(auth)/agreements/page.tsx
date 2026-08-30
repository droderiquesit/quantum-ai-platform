import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = { title: "Agreements" };

const AGREEMENTS = [
  {
    href: "/legal/terms",
    name: "Terms of Service",
    summary: "The contract governing use of the platform.",
  },
  {
    href: "/legal/privacy",
    name: "Privacy Policy",
    summary: "What is collected, why, and for how long it is kept.",
  },
  {
    href: "/legal/risk-disclosures",
    name: "Risk Disclosures",
    summary: "What paper-trading research can and cannot tell you about live markets.",
  },
] as const;

/**
 * The documents an account has agreed to, and the rule that makes the record
 * meaningful: acceptance is stored against the exact version that was on
 * screen. Without that, "the user accepted the terms" is a claim about
 * whichever text exists today, which is no claim at all.
 */
export default function AgreementsPage() {
  return (
    <div className="flex flex-col gap-4">
      <header>
        <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">Agreements</h1>
        <p className="mt-1 text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
          Every Algorik account has accepted these three documents.
        </p>
      </header>

      <ul className="flex flex-col gap-3">
        {AGREEMENTS.map((agreement) => (
          <li key={agreement.href} className="border border-[color:var(--color-border)] p-3">
            <Link href={agreement.href} className="text-[13px] font-medium text-[color:var(--color-ink)] underline">
              {agreement.name}
            </Link>
            <p className="mt-1 text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
              {agreement.summary}
            </p>
          </li>
        ))}
      </ul>

      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        Each document is versioned, and your acceptance is recorded against the version that was in
        front of you at the time. When a document changes, the platform asks you to accept the new
        version before continuing — the earlier acceptance stays on record against the text it
        actually applied to.
      </p>

      <Link href="/sign-in" className="text-[12px] text-[color:var(--color-ink-dim)] underline hover:text-[color:var(--color-ink)]">
        Back to sign in
      </Link>
    </div>
  );
}
