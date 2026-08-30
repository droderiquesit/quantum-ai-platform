import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = { title: "Account locked" };

/**
 * Shown after repeated failed sign-in attempts lock an account.
 *
 * The threshold and the delay are deliberately not stated: publishing either
 * hands an attacker the exact budget to spend per account per interval, and
 * the legitimate owner does not need the numbers — they need to know it will
 * pass on its own and that resetting the password is the faster way back in.
 */
export default function AccountLockedPage() {
  return (
    <div className="flex flex-col gap-4">
      <header>
        <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">Account locked</h1>
      </header>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        Repeated failed sign-in attempts lock an account for a while. The lock lifts on its own
        after a delay; continuing to guess extends it.
      </p>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        If you are not sure of the password, resetting it is the reliable way back in.
      </p>
      <Link href="/forgot-password" className="btn w-full" data-variant="primary">
        Reset password
      </Link>
    </div>
  );
}
