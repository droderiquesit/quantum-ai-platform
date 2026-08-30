"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "./icons";

/**
 * Who is signed in, and the way out.
 *
 * Rendered only when the deployment requires authentication and a session
 * exists; the open-console deployments never see it. Sign-out calls the
 * server first and navigates after: clearing state client-side before the
 * server has revoked would leave a live session behind a signed-out screen.
 */
interface SessionUser {
  readonly email: string;
  readonly displayName: string | null;
}

export function AccountMenu() {
  const [user, setUser] = useState<SessionUser | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const container = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetch("/api/auth/session", { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : null))
      .then((body) => {
        if (!cancelled && body?.status === "authenticated") setUser(body.session.user);
      })
      .catch(() => {
        /* an open deployment or a failed read: render nothing */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const onAway = (event: MouseEvent) => {
      if (container.current && !container.current.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onAway);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onAway);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const signOut = useCallback(async () => {
    setBusy(true);
    try {
      const csrf = await fetch("/api/auth/csrf", { cache: "no-store" }).then((r) => r.json());
      await fetch("/api/auth/sign-out", {
        method: "POST",
        headers: { "x-algorik-csrf": csrf.token },
      });
    } finally {
      // A hard navigation on purpose, not router.push: crossing the auth
      // boundary must tear down every in-memory poll, stream and cached
      // resource this tab holds, and only a full document load guarantees
      // that. Navigate even if revocation failed — the gateway refuses the
      // dead session either way, and the person is not trapped on a console
      // that half-works.
      // eslint-disable-next-line @next/next/no-location-assign-relative-destination
      window.location.assign("/welcome");
    }
  }, []);

  if (user === null) return null;

  const label = user.displayName || user.email;

  return (
    <div className="relative" ref={container}>
      <button
        type="button"
        className="flex size-11 items-center justify-center rounded-xl bg-panel border border-border hover:bg-border/50 transition-colors"
        data-testid="account-menu"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => setOpen((value) => !value)}
        title={user.email}
      >
        <span className="flex w-8 h-8 items-center justify-center rounded-full bg-gradient-to-br from-emerald-500 to-indigo-500 text-white text-sm font-bold">
          {label.slice(0, 1).toUpperCase()}
        </span>
      </button>
      {open ? (
        <div
          role="menu"
          className="absolute right-0 top-[52px] z-[80] w-64 mt-2 rounded-2xl bg-panel border border-border shadow-2xl overflow-hidden"
        >
          <div className="p-4 border-b border-border">
            <p className="truncate text-sm font-semibold text-text">{label}</p>
            <p className="truncate text-xs text-muted">{user.email}</p>
          </div>
          <button
            type="button"
            role="menuitem"
            className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm text-text hover:bg-border/40"
            data-testid="sign-out"
            onClick={() => void signOut()}
            disabled={busy}
          >
            <Icon name="log-out" className="w-4 h-4" />
            {busy ? "Signing out…" : "Sign out"}
          </button>
        </div>
      ) : null}
    </div>
  );
}
