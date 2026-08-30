"use client";

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

/**
 * Registers the service worker, and offers the install the platform allows.
 *
 * Android and desktop Chrome fire `beforeinstallprompt`, which can be deferred
 * and replayed from a control; iOS Safari fires nothing and installs only from
 * its own share sheet. Both are handled, and neither is faked: on iOS the
 * button explains where the real control is rather than pretending to be it.
 *
 * The environment questions — is this already installed, is this iOS Safari,
 * has this person said no before — are answered through `useSyncExternalStore`
 * rather than by an effect that sets state on mount. That is not style: this
 * component renders on the server too, and the store's server snapshot is the
 * one that hides the offer, so the banner cannot flash on during hydration and
 * then vanish.
 */

/** The event Chromium fires; not in the DOM lib because it is not standard. */
interface InstallPromptEvent extends Event {
  prompt(): Promise<void>;
  readonly userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}

const DISMISSED = "algorik.install.dismissed";

/** Nothing to subscribe to: these facts do not change within a page view. */
function inert(): () => void {
  return () => {};
}

/** True when the app is running from a home screen rather than a browser tab. */
function useStandalone(): boolean {
  return useSyncExternalStore(
    (onChange) => {
      const query = window.matchMedia("(display-mode: standalone)");
      query.addEventListener("change", onChange);
      return () => query.removeEventListener("change", onChange);
    },
    () =>
      window.matchMedia("(display-mode: standalone)").matches ||
      // iOS marks an installed launch here and implements no `display-mode`.
      (window.navigator as Navigator & { standalone?: boolean }).standalone === true,
    () => false,
  );
}

/** True for iOS Safari, the one browser that installs from its share sheet. */
function useIosSafari(): boolean {
  return useSyncExternalStore(
    inert,
    () => {
      const agent = window.navigator.userAgent;
      // An iPad on recent iPadOS reports itself as a Macintosh; the touch
      // handler is what separates it from an actual desktop.
      const isIos =
        /iPad|iPhone|iPod/.test(agent) || (agent.includes("Macintosh") && "ontouchend" in window);
      const isSafari = /Safari/.test(agent) && !/CriOS|FxiOS|EdgiOS|Chrome/.test(agent);
      return isIos && isSafari;
    },
    () => false,
  );
}

/** Whether this browser has already been told no. */
function useDismissedBefore(): boolean {
  return useSyncExternalStore(
    inert,
    () => {
      try {
        return window.localStorage.getItem(DISMISSED) === "true";
      } catch {
        // Storage can be denied outright. Treat that as "not dismissed": it is
        // a preference, and losing it should re-offer rather than suppress.
        return false;
      }
    },
    () => true,
  );
}

export function InstallApp() {
  const standalone = useStandalone();
  const ios = useIosSafari();
  const dismissedBefore = useDismissedBefore();

  const [prompt, setPrompt] = useState<InstallPromptEvent | null>(null);
  const [installedNow, setInstalledNow] = useState(false);
  const [dismissedNow, setDismissedNow] = useState(false);

  useEffect(() => {
    if ("serviceWorker" in navigator && window.isSecureContext) {
      navigator.serviceWorker.register("/sw.js").catch(() => {
        // A failed registration costs the offline page and nothing else. The
        // console works entirely without it, so this is not worth an alert.
      });
    }
  }, []);

  useEffect(() => {
    const onPrompt = (event: Event) => {
      event.preventDefault();
      setPrompt(event as InstallPromptEvent);
    };
    const onInstalled = () => {
      setInstalledNow(true);
      setPrompt(null);
    };
    window.addEventListener("beforeinstallprompt", onPrompt);
    window.addEventListener("appinstalled", onInstalled);
    return () => {
      window.removeEventListener("beforeinstallprompt", onPrompt);
      window.removeEventListener("appinstalled", onInstalled);
    };
  }, []);

  const dismiss = useCallback(() => {
    setDismissedNow(true);
    try {
      window.localStorage.setItem(DISMISSED, "true");
    } catch {
      /* the offer simply returns next load */
    }
  }, []);

  const install = useCallback(async () => {
    if (prompt === null) return;
    await prompt.prompt();
    await prompt.userChoice;
    setPrompt(null);
  }, [prompt]);

  if (standalone || installedNow || dismissedBefore || dismissedNow) return null;
  if (prompt === null && !ios) return null;

  return (
    <aside
      className="fixed inset-x-2 bottom-[76px] z-[70] flex items-start gap-3 rounded-2xl border border-border bg-panel px-3 py-2.5 shadow-2xl md:bottom-3 lg:hidden"
      style={{ marginBottom: "env(safe-area-inset-bottom, 0px)" }}
      aria-label="Install this console"
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-[12.5px] font-medium text-[color:var(--color-ink)]">
          Install Algorik on this device
        </span>
        <span className="text-[11px] leading-relaxed text-[color:var(--color-ink-dim)]">
          {ios
            ? "In Safari, tap Share and then Add to Home Screen. It opens full screen and keeps working — offline it shows no figures, only that it cannot reach the platform."
            : "Opens full screen from your home screen. It stores no positions or limits on the device, so offline it shows nothing rather than something stale."}
        </span>
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        {prompt !== null ? (
          <button
            type="button"
            className="btn"
            data-variant="primary"
            onClick={() => void install()}
          >
            Install
          </button>
        ) : null}
        <button
          type="button"
          className="btn"
          data-variant="ghost"
          onClick={dismiss}
          aria-label="Dismiss the install offer"
        >
          ✕
        </button>
      </div>
    </aside>
  );
}
