"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

/**
 * The header's session-aware call to action, and the only client component
 * in the marketing group.
 *
 * It asks `GET /api/auth/session` exactly once per mount and renders one of
 * three things: a fixed-width placeholder while the answer is unknown (so
 * the header never shifts when it arrives), the sign-in pair for a visitor,
 * or a single "Open Algorik" for someone already signed in.
 *
 * A failure to reach the endpoint — including a deployment where the route
 * does not exist yet — reads as signed out, because the honest default for a
 * public page is the public pair of buttons. Nothing is retried and nothing
 * is invented: the response deliberately carries no portfolio data, and this
 * component renders none.
 */

type CtaState = "unknown" | "unauthenticated" | "authenticated";

export function AuthCta() {
  const [state, setState] = useState<CtaState>("unknown");

  useEffect(() => {
    let cancelled = false;

    async function ask(): Promise<void> {
      let next: CtaState = "unauthenticated";
      try {
        const response = await fetch("/api/auth/session", {
          method: "GET",
          credentials: "same-origin",
          cache: "no-store",
          headers: { accept: "application/json" },
        });
        if (response.ok) {
          const body: unknown = await response.json();
          if (
            typeof body === "object" &&
            body !== null &&
            (body as { status?: unknown }).status === "authenticated"
          ) {
            next = "authenticated";
          }
        }
      } catch {
        // Unreachable or not yet served: the signed-out pair is the honest
        // default for a public page, not an error state to surface here.
      }
      if (!cancelled) setState(next);
    }

    void ask();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    // The container reserves the width of the widest state so the header
    // does not reflow when the session answer lands.
    <span className="flex min-w-[176px] items-center justify-end gap-2">
      {state === "unknown" ? (
        <span aria-hidden="true" className="inline-block h-[26px] w-full" />
      ) : state === "authenticated" ? (
        <Link href="/" className="btn" data-variant="primary" data-testid="open-algorik">
          Open Algorik
        </Link>
      ) : (
        <>
          <Link href="/sign-in" className="btn" data-variant="ghost">
            Sign in
          </Link>
          <Link href="/sign-up" className="btn" data-variant="primary">
            Get started
          </Link>
        </>
      )}
    </span>
  );
}
