"use client";

import { useCallback, useEffect, useRef, useState } from "react";

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
        className="btn"
        data-variant="ghost"
        data-testid="account-menu"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => setOpen((value) => !value)}
        title={user.email}
      >
        <span className="flex h-[18px] w-[18px] items-center justify-center rounded-full bg-[color:var(--color-brand-primary-muted)] text-[10px] font-semibold text-[color:var(--color-brand-primary)]">
          {label.slice(0, 1).toUpperCase()}
        </span>
        <span className="num hidden max-w-[140px] truncate text-[10.5px] md:inline">{label}</span>
      </button>
      {open ? (
        <div
          role="menu"
          className="absolute right-0 top-[30px] z-[80] min-w-[200px] border border-[color:var(--color-border-strong)] bg-[color:var(--color-surface)] shadow-lg"
        >
          <div className="border-b border-[color:var(--color-border)] px-3 py-2">
            <p className="truncate text-[12px] text-[color:var(--color-text-primary)]">{label}</p>
            <p className="num truncate text-[10px] text-[color:var(--color-text-faint)]">{user.email}</p>
          </div>
          <button
            type="button"
            role="menuitem"
            className="block w-full px-3 py-2 text-left text-[12px] text-[color:var(--color-text-primary)] hover:bg-[color:var(--color-surface-elevated)]"
            data-testid="sign-out"
            onClick={() => void signOut()}
            disabled={busy}
          >
            {busy ? "Signing out…" : "Sign out"}
          </button>
        </div>
      ) : null}
    </div>
  );
}
