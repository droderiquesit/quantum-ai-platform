"use client";

/**
 * The last resort: the root layout itself threw, so no chrome exists to render
 * inside.
 *
 * Next replaces the whole document here, which is why this file declares its
 * own `<html>` and `<body>` and why every rule below is an inline style — the
 * stylesheet the root layout imports is exactly the thing that may not have
 * loaded, and a fallback that depends on the failed path is not a fallback.
 * Without this file the same failure paints nothing at all, and a blank tab
 * cannot be told apart from a console that never deployed.
 *
 * `PAPER TRADING` is stated here in so many words. `(portal)/error.tsx` can
 * rely on `AppShell` still being mounted above it; this one cannot rely on
 * anything, and `.claude/rules/domains/frontend.md` requires the declaration
 * wherever posture is shown. It is a static literal rather than a reading of
 * `live_capable`, because the fact it states is a property of this console —
 * no control it ships can submit an order — and that is true whether or not
 * the platform ever answered.
 */
export default function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <html lang="en">
      <body
        style={{
          margin: 0,
          background: "#050709",
          color: "#e6e9ef",
          fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        }}
      >
        <main style={{ maxWidth: "70ch", padding: "48px 24px" }} data-testid="global-error">
          <p
            style={{
              margin: "0 0 20px",
              fontSize: 11,
              fontWeight: 700,
              letterSpacing: "0.22em",
              color: "#a78bfa",
            }}
            data-testid="paper-trading-declaration"
          >
            PAPER TRADING
          </p>
          <h1 style={{ margin: "0 0 12px", fontSize: 16, fontWeight: 500 }}>
            The console failed to start.
          </h1>
          <p style={{ margin: "0 0 12px", fontSize: 13, lineHeight: 1.7, color: "#9aa3b2" }}>
            This is a fault in the console itself, not a report about the platform: nothing here
            got far enough to ask the platform anything. Simulated execution only — no control
            this console ships can submit a live order, and that is true of this page too.
          </p>
          {error.digest ? (
            <p style={{ margin: "0 0 20px", fontSize: 12, color: "#6b7280" }}>
              Quote this digest when reporting it: <code>{error.digest}</code>
            </p>
          ) : null}
          <button
            type="button"
            onClick={reset}
            style={{
              border: "1px solid #2a3140",
              background: "transparent",
              color: "inherit",
              font: "inherit",
              fontSize: 12,
              padding: "8px 14px",
              cursor: "pointer",
            }}
          >
            Reload the console
          </button>
        </main>
      </body>
    </html>
  );
}
