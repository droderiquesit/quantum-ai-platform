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
 * ours is the *data*: Algorik's eight sections and thirty-four destinations,
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
      <div
        id="sidebarOverlay"
        data-testid="sidebar-overlay"
        onClick={onClose}
        className={`fixed inset-0 bg-black/50 z-40 lg:hidden transition-opacity duration-300 ${
          mobileOpen ? "opacity-100" : "opacity-0 pointer-events-none"
        }`}
        aria-hidden="true"
      />
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
                    {item.simulated ? (
                      <span className="ml-auto text-[9px] font-bold px-1.5 py-0.5 rounded-md bg-secondary/15 text-secondary nav-text">
                        SIM
                      </span>
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
