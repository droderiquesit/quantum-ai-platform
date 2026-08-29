"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { NAV } from "@/lib/nav";

/**
 * The rail, at desk width, and the same map as a scrolling bar below it.
 *
 * Both are rendered and one is hidden per breakpoint rather than one being
 * swapped for the other, so a link is always present in the accessibility tree
 * for the layout the reader is actually in.
 */
export function NavRail() {
  const pathname = usePathname();
  return (
    <nav
      aria-label="Sections"
      className="hidden w-[212px] shrink-0 flex-col gap-4 overflow-y-auto border-r border-[color:var(--color-line)] bg-[color:var(--color-sunken)] py-3 lg:flex"
    >
      {NAV.map((group) => (
        <div key={group.label} className="flex flex-col">
          <span className="eyebrow px-3 pb-1.5">{group.label}</span>
          {group.items.map((item) => {
            const active = pathname === item.href;
            return (
              <Link
                key={item.href}
                href={item.href}
                aria-current={active ? "page" : undefined}
                title={item.description}
                className={`flex items-center gap-2.5 border-l-2 px-3 py-[5px] text-[12.5px] transition-colors ${
                  active
                    ? "border-[color:var(--color-accent)] bg-[color:var(--color-raised)] text-[color:var(--color-ink)]"
                    : "border-transparent text-[color:var(--color-ink-dim)] hover:bg-[color:var(--color-raised)] hover:text-[color:var(--color-ink)]"
                }`}
              >
                <span className="num w-[20px] text-[10px] text-[color:var(--color-ink-faint)]">
                  {item.mark}
                </span>
                <span className="truncate">{item.label}</span>
              </Link>
            );
          })}
        </div>
      ))}
    </nav>
  );
}

export function NavBar() {
  const pathname = usePathname();
  return (
    <nav
      aria-label="Sections (compact)"
      className="hidden shrink-0 items-stretch gap-0 overflow-x-auto border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] md:flex lg:hidden"
    >
      {NAV.flatMap((group) => group.items).map((item) => {
        const active = pathname === item.href;
        return (
          <Link
            key={item.href}
            href={item.href}
            aria-current={active ? "page" : undefined}
            title={item.description}
            className={`flex items-center whitespace-nowrap border-b-2 px-3 py-2 text-[12px] ${
              active
                ? "border-[color:var(--color-accent)] text-[color:var(--color-ink)]"
                : "border-transparent text-[color:var(--color-ink-dim)]"
            }`}
          >
            {item.label}
          </Link>
        );
      })}
    </nav>
  );
}
