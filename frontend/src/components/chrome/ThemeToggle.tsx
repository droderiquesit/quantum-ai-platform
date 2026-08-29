"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * The dark/light switch.
 *
 * The stored choice is applied to <html data-theme> by an inline script in the
 * root layout *before first paint* — this component only reflects and updates
 * it. If the attribute were set here, after hydration, every load would paint
 * dark first and then flash to light for anyone who chose light, once per
 * navigation, forever.
 *
 * The attribute is the store and this component subscribes to it, so two
 * instances of the toggle (or anything else that themes) can never disagree
 * about which theme is on. Dark is the default and stores no attribute;
 * storage can be denied, in which case the choice holds for the session and
 * is simply not remembered.
 */
const KEY = "peos.theme";

function subscribe(onChange: () => void): () => void {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  return () => observer.disconnect();
}

function snapshot(): "dark" | "light" {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

export function ThemeToggle() {
  const theme = useSyncExternalStore(subscribe, snapshot, () => "dark");

  const flip = useCallback(() => {
    const next = theme === "dark" ? "light" : "dark";
    if (next === "light") {
      document.documentElement.dataset.theme = "light";
    } else {
      delete document.documentElement.dataset.theme;
    }
    try {
      window.localStorage.setItem(KEY, next);
    } catch {
      /* the choice holds for this session only */
    }
  }, [theme]);

  return (
    <button
      type="button"
      className="btn"
      data-variant="ghost"
      data-testid="theme-toggle"
      onClick={flip}
      aria-pressed={theme === "light"}
      aria-label={theme === "dark" ? "Switch to the light theme" : "Switch to the dark theme"}
      title={theme === "dark" ? "Light theme" : "Dark theme"}
    >
      {theme === "dark" ? "☀" : "☾"}
    </button>
  );
}
