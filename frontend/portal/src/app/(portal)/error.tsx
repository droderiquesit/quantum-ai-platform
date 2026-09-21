"use client";

import Link from "next/link";
import { useEffect } from "react";
import { StateBlock } from "@/components/data/States";

/**
 * What a console page becomes when it throws instead of rendering.
 *
 * Without this file React unmounts the whole tree and Next serves its
 * production fallback — a blank document reading "Application error: a
 * client-side exception has occurred". The chrome goes with it: the
 * navigation, the connection indicator, and the `PAPER TRADING` declaration
 * that `.claude/rules/domains/frontend.md` requires wherever posture is shown.
 * A blank page is also indistinguishable from a console that failed to deploy,
 * which sends an operator to the wrong problem during the exact minutes they
 * can least afford it.
 *
 * This is not hypothetical here. `tests/support/platform.ts` records it
 * happening once already: a `/risk` fixture with a plausible-but-wrong shape
 * "crashed the console to its global error page rather than rendering", and
 * the real platform can produce an unexpected shape whenever a subsystem is
 * recomposed.
 *
 * Because `error.tsx` sits inside the `(portal)` segment, React replaces only
 * the page beneath `AppShell` — the banner, the navigation and the status bar
 * stay mounted. So the reader keeps the posture declaration and a way out.
 *
 * It deliberately does **not** say the backend is unavailable. A page that
 * threw and a platform that cannot be reached are different faults with
 * different remedies, and the panels already state the second one themselves
 * (`ResourceView` in `@/components/data/States`). Claiming the API is down
 * because this console has a bug would be the console lying about the
 * platform.
 */
export default function PortalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    // The browser console is the only sink a static export has. The message is
    // the app's own; no request body, no credential and no session value
    // passes through here.
    console.error("A console page stopped rendering.", error);
  }, [error]);

  return (
    <div className="flex flex-col gap-3 p-6" data-testid="portal-error">
      <StateBlock
        tone="bad"
        label="page failed"
        headline="This page stopped rendering."
        action={
          <div className="flex items-center gap-2">
            <button type="button" className="btn" onClick={reset}>
              Try this page again
            </button>
            <Link className="btn" href="/">
              Back to the dashboard
            </Link>
          </div>
        }
      >
        <p>
          The fault is in this console, not necessarily in the platform: a page that throws and a
          platform that cannot be reached are different problems. Other pages are unaffected, and
          the declaration above still holds — nothing here can send a live order.
        </p>
        {error.digest ? (
          <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
            Quote this digest when reporting it: <code>{error.digest}</code>
          </p>
        ) : null}
      </StateBlock>
    </div>
  );
}
