"use client";

import { useEffect } from "react";

/**
 * Re-asserts the stored theme after hydration.
 *
 * The boot script in the root layout stamps <html data-theme> before first
 * paint, which is what stops the dark-flash. React 19's hydration then
 * reconciles the root element against what the server rendered — and an
 * attribute the render never declared can be removed in that pass,
 * suppressHydrationWarning notwithstanding. Observed as: the boot script
 * sets "light", and ~100ms later the attribute is gone, on a page whose
 * reload was supposed to remember the choice.
 *
 * Nothing else restores it: the toggle only observes the attribute, by
 * design, so a stripped attribute stayed stripped. This effect runs once
 * after hydration and stamps the stored choice again. On the loads where
 * the reconciler leaves the attribute alone, the set is idempotent.
 */
export function ThemeSync() {
  useEffect(() => {
    try {
      document.documentElement.dataset.theme =
        window.localStorage.getItem("algorik.theme") === "light" ? "light" : "dark";
    } catch {
      /* storage denied: the boot script's pre-paint stamp stands */
    }
  }, []);
  return null;
}
