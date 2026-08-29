import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = { title: "Offline" };

/**
 * What the installed app shows when it is opened with no connection.
 *
 * Deliberately empty of figures. The service worker caches no platform answer,
 * so there is nothing here to show and nothing here that could be mistaken for
 * current. The page exists to say that plainly rather than to fill the screen.
 */
export default function OfflinePage() {
  return (
    <div className="flex min-h-full flex-col items-start gap-3 p-5">
      <span className="eyebrow">offline</span>
      <h2 className="text-[18px] font-semibold text-[color:var(--color-ink)]">
        This device has no connection to the platform.
      </h2>
      <p className="max-w-[62ch] text-[13px] leading-relaxed text-[color:var(--color-ink-dim)]">
        Nothing is shown because nothing is known. This application stores no positions, no fills,
        no limits and no halt state on the device — a figure kept from the last session would look
        exactly like a current one, and on a trading surface that is worse than a blank screen.
      </p>
      <p className="max-w-[62ch] text-[13px] leading-relaxed text-[color:var(--color-ink-dim)]">
        Reconnect and the console will read the platform again. Until then, the only thing it can
        honestly tell you is that it does not know.
      </p>
      <p className="max-w-[62ch] text-[12px] leading-relaxed text-[color:var(--color-ink-faint)]">
        The platform remains paper trading whether this device can reach it or not. No control in
        this application submits a live order, offline or on.
      </p>
      <Link className="btn" data-variant="primary" href="/">
        Try again
      </Link>
    </div>
  );
}
