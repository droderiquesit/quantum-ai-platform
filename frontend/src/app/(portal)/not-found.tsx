import Link from "next/link";
import { NAV_ITEMS } from "@/lib/nav";

export default function NotFound() {
  return (
    <div className="flex flex-col gap-3 p-6">
      <span className="eyebrow">404</span>
      <h2 className="text-[15px] font-medium">This console has no page at that address.</h2>
      <p className="max-w-[70ch] text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
        The command centre is still paper trading only, whichever page you land on. Everything it
        serves is listed below.
      </p>
      <ul className="flex flex-col gap-1 pt-2">
        {NAV_ITEMS.map((item) => (
          <li key={item.href}>
            <Link
              href={item.href}
              className="text-[12.5px] text-[color:var(--color-accent)] underline underline-offset-2"
            >
              {item.label}
            </Link>
            <span className="ml-2 text-[11px] text-[color:var(--color-ink-faint)]">
              {item.description}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
