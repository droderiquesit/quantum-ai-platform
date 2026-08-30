import type { Metadata } from "next";
import Link from "next/link";
import { CtaLink, PageIntro } from "../ui";

export const metadata: Metadata = {
  title: "Contact",
  description: "How to reach the Algorik team.",
};

/**
 * A thin page: the contact details themselves live on the company page so
 * there is exactly one place to keep them true. This route exists because
 * /contact is where people look.
 */
export default function ContactPage() {
  return (
    <PageIntro
      eyebrow="Contact"
      title="Contact lives on the company page."
      lede="One address, kept in one place, so it can only be right or wrong once."
    >
      <div className="mt-8 flex flex-wrap items-center gap-4">
        <CtaLink href="/company#contact" variant="primary">
          Go to contact
        </CtaLink>
        <Link
          href="/company"
          className="text-[13px] text-[color:var(--color-ink-dim)] underline underline-offset-2 hover:text-[color:var(--color-ink)]"
        >
          About Algorik
        </Link>
      </div>
    </PageIntro>
  );
}
