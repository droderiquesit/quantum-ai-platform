"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { AlgorikMark } from "@algorik/brand";
import { NAV } from "@/lib/nav";
import { Icon, ITEM_ICON } from "./icons";

/**
 * The sidebar, in the licensed template's structure (ADR 0015).
 *
 * The markup classes are the template's own — `sidebar`, `logo-section`,
 * `nav-section-title`, `nav-item`, `nav-text`, `live-dot` — so its CSS layer
 * styles this untouched: collapse to an icon rail on desktop, off-canvas
 * behind an overlay on mobile, hover flyout labels when collapsed. What is
 * ours is the *data*: Algorik's ten sections and forty destinations,
 * the real hrefs, and the active state from the router rather than a build
 * step.
 *
 * Collapse state persists per browser under "algorik.sidebar". Mobile open
 * state deliberately does not: an off-canvas menu that reopens itself on the
 * next visit is a bug report, not a preference.
 */
const COLLAPSE_KEY = "algorik.sidebar";

/**
 * Collapse state lives in localStorage and is read through an external-store
 * subscription: the server snapshot is "open", so hydration matches, and the
 * first client render corrects to the stored preference without an effect
 * that sets state on mount (the lint rule exists because that pattern
 * cascades renders).
 */
const collapseListeners = new Set<() => void>();

function subscribeCollapse(onChange: () => void): () => void {
  collapseListeners.add(onChange);
  window.addEventListener("storage", onChange);
  return () => {
    collapseListeners.delete(onChange);
    window.removeEventListener("storage", onChange);
  };
}

function readCollapsed(): boolean {
  try {
    return window.localStorage.getItem(COLLAPSE_KEY) === "collapsed";
  } catch {
    return false;
  }
}

export function useSidebar() {
  const collapsed = useSyncExternalStore(subscribeCollapse, readCollapsed, () => false);
  const [mobileOpen, setMobileOpen] = useState(false);

  const toggleCollapsed = useCallback(() => {
    try {
      window.localStorage.setItem(COLLAPSE_KEY, collapsed ? "open" : "collapsed");
    } catch {
      /* preference not persisted; the toggle still works via the notify */
    }
    for (const listener of collapseListeners) listener();
  }, [collapsed]);

  return { collapsed, mobileOpen, setMobileOpen, toggleCollapsed };
}

export function Sidebar({
  collapsed,
  mobileOpen,
  onClose,
}: {
  collapsed: boolean;
  mobileOpen: boolean;
  onClose: () => void;
}) {
  const pathname = usePathname();

  // A route change closes the mobile sidebar, or tapping a link leaves the
  // off-canvas panel covering the page it navigated to.
  useEffect(() => {
    onClose();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- close on path change only
  }, [pathname]);

  return (
    <>
      {/* In the DOM only while open: an overlay that is merely transparent
          still reads as present to assistive tech and to the test harness,
          and "closed" must be a fact of the tree, not of a style. */}
      {mobileOpen ? (
        <div
          id="sidebarOverlay"
          data-testid="sidebar-overlay"
          onClick={onClose}
          className="fixed inset-0 bg-black/50 z-40 lg:hidden"
          aria-hidden="true"
        />
      ) : null}
      <aside
        className={`sidebar ${collapsed ? "collapsed" : ""} ${mobileOpen ? "mobile-open" : ""}`}
        data-testid="sidebar"
        aria-label="Sections"
      >
        <div className="logo-section flex items-center gap-3 p-5 border-b border-border shrink-0">
          <span className="flex size-10 items-center justify-center rounded-xl bg-accent/10 text-accent shrink-0">
            <AlgorikMark size={26} title="Algorik" />
          </span>
          <div className="logo-text flex-1 min-w-0">
            <div className="text-base font-bold text-text leading-tight truncate">Algorik</div>
            <div className="text-[10px] font-semibold uppercase tracking-[1.5px] text-muted truncate">
              Paper trading
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close the navigation"
            className="lg:hidden! flex w-8 h-8 items-center justify-center rounded-lg text-muted hover:bg-border/50"
          >
            <Icon name="x" className="w-4 h-4" />
          </button>
        </div>

        <nav className="py-4 flex-1 overflow-y-auto hide-scrollbar">
          {NAV.map((group) => (
            <div key={group.label}>
              <div className="nav-section-title px-5 pt-4 pb-2 mt-2 text-[11px] font-bold uppercase tracking-[1.5px] text-muted">
                {group.label}
              </div>
              {group.items.map((item) => {
                const active = pathname === item.href;
                return (
                  <Link
                    key={item.href}
                    href={item.href}
                    data-label={item.label}
                    aria-current={active ? "page" : undefined}
                    title={item.description}
                    className={`nav-item ${active ? "active" : ""} flex items-center gap-3 mx-3 my-0.5 px-4 py-3 rounded-xl text-sm font-medium text-muted`}
                  >
                    <Icon name={ITEM_ICON[item.href] ?? "layout-dashboard"} />
                    <span className="nav-text">{item.label}</span>
                    {item.href === "/signals" ? (
                      <span className="live-dot ml-auto nav-text" aria-hidden="true" />
                    ) : null}
                  </Link>
                );
              })}
            </div>
          ))}
        </nav>

        <div className="user-section mt-auto p-4 border-t border-border shrink-0">
          <p className="nav-text text-[10.5px] leading-relaxed text-muted">
            Simulated execution only — no control here submits a live order.
          </p>
        </div>
      </aside>
    </>
  );
}

/**
 * The phone's tab bar: the installed app's primary navigation, in the
 * portal's own design language — same tokens, same icons, same emerald
 * active state — so the "mobile app" is visibly the same product.
 *
 * Four primary surfaces plus Menu, which opens the full off-canvas sidebar:
 * a strip that tried to carry all forty destinations would carry none
 * of them reachably. Phones only; tablets and desktops keep the sidebar.
 */
const TABS = [
  { href: "/", label: "Dashboard", icon: "layout-dashboard" },
  { href: "/signals", label: "Opportunities", icon: "radio" },
  { href: "/portfolio", label: "Portfolio", icon: "wallet" },
  { href: "/risk", label: "Risk", icon: "shield-check" },
] as const;

export function MobileTabBar({ onMenu }: { onMenu: () => void }) {
  const pathname = usePathname();
  const inTabs = TABS.some((tab) => tab.href === pathname);

  return (
    <nav
      aria-label="Primary (phone)"
      data-testid="mobile-tab-bar"
      className="fixed inset-x-0 bottom-0 z-40 md:hidden glass border-t border-border"
      style={{ paddingBottom: "env(safe-area-inset-bottom, 0px)" }}
    >
      <div className="flex items-stretch">
        {TABS.map((tab) => {
          const active = pathname === tab.href;
          return (
            <Link
              key={tab.href}
              href={tab.href}
              aria-current={active ? "page" : undefined}
              className={`flex min-h-[56px] flex-1 flex-col items-center justify-center gap-1 text-[10px] font-semibold transition-colors ${
                active ? "text-accent" : "text-muted hover:text-text"
              }`}
            >
              <Icon name={tab.icon} className="w-5 h-5 shrink-0" />
              {tab.label}
            </Link>
          );
        })}
        <button
          type="button"
          onClick={onMenu}
          aria-label="All sections"
          data-testid="tab-bar-menu"
          className={`flex min-h-[56px] flex-1 flex-col items-center justify-center gap-1 text-[10px] font-semibold transition-colors ${
            !inTabs ? "text-accent" : "text-muted hover:text-text"
          }`}
        >
          <Icon name="menu" className="w-5 h-5 shrink-0" />
          Menu
        </button>
      </div>
    </nav>
  );
}
