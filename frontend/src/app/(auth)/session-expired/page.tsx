import type { Metadata } from "next";
import Link from "next/link";
import { safeRedirect } from "@algorik/auth";

export const metadata: Metadata = { title: "Session expired" };

/**
 * Where the platform sends a browser whose session has lapsed.
 *
 * The `next` parameter carries where the person was, so signing back in
 * returns them there — but it passes through `safeRedirect` before it is put
 * in a link, because a parameter honoured verbatim would let a crafted URL
 * walk someone from a real Algorik page to anywhere at all.
 */
export default async function SessionExpiredPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const rawNext = typeof params.next === "string" ? params.next : null;
  const destination = safeRedirect(rawNext, "/");
  const signInHref =
    destination === "/" ? "/sign-in" : `/sign-in?next=${encodeURIComponent(destination)}`;

  return (
    <div className="flex flex-col gap-4">
      <header>
        <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">Session expired</h1>
      </header>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        Your session ended — sessions lapse after inactivity so an unattended screen cannot keep
        acting under your name. Sign in again and you will be returned to where you were.
      </p>
      <Link href={signInHref} className="btn w-full" data-variant="primary">
        Sign in again
      </Link>
    </div>
  );
}
