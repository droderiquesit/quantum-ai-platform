import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = { title: "Access denied" };

/**
 * Shown when a signed-in account asks for a surface its roles do not cover.
 *
 * It names no contact because who assigns roles is deployment-specific — a
 * hardcoded email here would be wrong on every deployment but the one it was
 * written for, and the person who issued the account is the one constant every
 * deployment has.
 */
export default function AccessDeniedPage() {
  return (
    <div className="flex flex-col gap-4">
      <header>
        <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">Access denied</h1>
      </header>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        The account you are signed in with does not carry the role that page requires. This is a
        property of the account, not a fault in the page — signing out and back in will not change
        it.
      </p>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        Roles are assigned per deployment, so if you believe you need this access, ask the operator
        who issued your account.
      </p>
      <Link href="/" className="btn w-full" data-variant="primary">
        Back to the dashboard
      </Link>
    </div>
  );
}
