"use client";

import { useCallback, useSyncExternalStore } from "react";
import { Icon } from "./icons";

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
const KEY = "algorik.theme";

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
    // Stamped explicitly in both directions: the template's dark: utilities
    // match an ancestor attribute, so "dark by absence" would leave them off.
    document.documentElement.dataset.theme = next;
    try {
      window.localStorage.setItem(KEY, next);
    } catch {
      /* the choice holds for this session only */
    }
  }, [theme]);

  return (
    <button
      type="button"
      className="flex size-11 items-center justify-center rounded-xl bg-panel border border-border text-text hover:bg-border/50 transition-colors"
      data-testid="theme-toggle"
      onClick={flip}
      aria-pressed={theme === "light"}
      aria-label={theme === "dark" ? "Switch to the light theme" : "Switch to the dark theme"}
      title={theme === "dark" ? "Light theme" : "Dark theme"}
    >
      <Icon name={theme === "dark" ? "sun" : "moon"} />
    </button>
  );
}
