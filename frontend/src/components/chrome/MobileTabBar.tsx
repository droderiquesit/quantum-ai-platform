"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useState } from "react";
import { NAV, NAV_ITEMS } from "@/lib/nav";
import { ThemeToggle } from "./ThemeToggle";

/**
 * The phone's navigation: five destinations at the thumb, everything else in a
 * sheet.
 *
 * A fifteen-item scrolling strip is a desktop rail wearing a phone's clothes —
 * the items past the fold are unreachable without a horizontal scroll nobody
 * discovers. So the bar carries the four surfaces an operator opens the app to
 * check, and `More` opens the full map grouped exactly as the desk rail groups
 * it, so the two layouts teach the same structure.
 *
 * The bar sits above the home indicator via `env(safe-area-inset-bottom)`. A
 * control an iPhone's gesture area sits on top of is a control that fires the
 * system gesture instead, and on this console one of those controls is next to
 * the kill switch.
 */

/** What the app opens to. Everything else is one tap further, in `More`. */
const PRIMARY = ["/", "/signals", "/portfolio", "/risk"] as const;

export function MobileTabBar() {
  const pathname = usePathname();
  // The route the sheet was opened on, rather than a plain boolean. A
  // navigation changes `pathname`, so the sheet closes itself without an
  // effect — and tapping a link inside it cannot leave it covering the page it
  // just navigated to.
  const [openedOn, setOpenedOn] = useState<string | null>(null);
  const open = openedOn === pathname;
  const setOpen = useCallback(
    (next: boolean) => setOpenedOn(next ? pathname : null),
    [pathname],
  );

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);

  const tabs = PRIMARY.map((href) => NAV_ITEMS.find((item) => item.href === href)).filter(
    (item): item is NonNullable<typeof item> => item !== undefined,
  );
  const inSheet = !PRIMARY.includes(pathname as (typeof PRIMARY)[number]);

  return (
    <>
      {open ? (
        <div className="fixed inset-0 z-[80] flex flex-col md:hidden" role="dialog" aria-modal="true" aria-label="All sections">
          <button
            type="button"
            className="flex-1 bg-black/60"
            onClick={() => setOpen(false)}
            aria-label="Close the section list"
          />
          <div
            className="max-h-[72dvh] overflow-y-auto border-t border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)]"
            style={{ paddingBottom: "calc(env(safe-area-inset-bottom, 0px) + 12px)" }}
          >
            <div className="sticky top-0 flex items-center justify-between gap-2 border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-4 py-2.5">
              <span className="text-[13px] font-medium text-[color:var(--color-ink)]">Sections</span>
              <span className="ml-auto flex items-center gap-1.5">
                {/* The phone header has no room for this; the sheet does. */}
                <ThemeToggle />
                <button type="button" className="btn" data-variant="ghost" onClick={() => setOpen(false)}>
                  Close
                </button>
              </span>
            </div>
            {NAV.map((group) => (
              <div key={group.label} className="flex flex-col border-b border-[color:var(--color-line)] py-2 last:border-b-0">
                <span className="eyebrow px-4 pb-1">{group.label}</span>
                {group.items.map((item) => (
                  <Link
                    key={item.href}
                    href={item.href}
                    aria-current={pathname === item.href ? "page" : undefined}
                    className={`flex min-h-[46px] items-center gap-3 px-4 py-2 ${
                      pathname === item.href
                        ? "bg-[color:var(--color-raised)] text-[color:var(--color-ink)]"
                        : "text-[color:var(--color-ink-dim)]"
                    }`}
                  >
                    <span className="num w-[22px] shrink-0 text-[10px] text-[color:var(--color-ink-faint)]">
                      {item.mark}
                    </span>
                    <span className="flex min-w-0 flex-col">
                      <span className="truncate text-[13px]">{item.label}</span>
                      <span className="truncate text-[10.5px] text-[color:var(--color-ink-faint)]">
                        {item.description}
                      </span>
                    </span>
                  </Link>
                ))}
              </div>
            ))}
          </div>
        </div>
      ) : null}

      <nav
        aria-label="Primary (compact)"
        data-testid="mobile-tab-bar"
        className="flex shrink-0 items-stretch border-t border-[color:var(--color-line)] bg-[color:var(--color-sunken)] md:hidden"
        style={{ paddingBottom: "env(safe-area-inset-bottom, 0px)" }}
      >
        {tabs.map((item) => {
          const active = pathname === item.href;
          return (
            <Link
              key={item.href}
              href={item.href}
              aria-current={active ? "page" : undefined}
              className={`flex min-h-[52px] flex-1 flex-col items-center justify-center gap-0.5 border-t-2 px-1 ${
                active
                  ? "border-[color:var(--color-accent)] text-[color:var(--color-ink)]"
                  : "border-transparent text-[color:var(--color-ink-dim)]"
              }`}
            >
              <span className="num text-[11px] tracking-[0.08em]">{item.mark}</span>
              <span className="truncate text-[10px]">{item.label.split(" ")[0]}</span>
            </Link>
          );
        })}
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          aria-label="All sections"
          className={`flex min-h-[52px] flex-1 flex-col items-center justify-center gap-0.5 border-t-2 px-1 ${
            inSheet
              ? "border-[color:var(--color-accent)] text-[color:var(--color-ink)]"
              : "border-transparent text-[color:var(--color-ink-dim)]"
          }`}
        >
          <span className="num text-[11px] tracking-[0.08em]">•••</span>
          <span className="truncate text-[10px]">More</span>
        </button>
      </nav>
    </>
  );
}
