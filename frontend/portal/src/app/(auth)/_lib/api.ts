/**
 * The auth pages' one way of talking to `/api/auth/*`.
 *
 * Centralised so that every page refuses the same way. The failure modes here
 * — proxy down, CSRF endpoint unreachable, a response that is not the contract
 * shape — would otherwise each be handled by whichever page hit them first,
 * with whichever wording that page's author invented. A person retrying a
 * sign-in and a person retrying a password reset should read the same sentence
 * when the same thing is wrong.
 *
 * Nothing here ever puts an email address, a password or a code into a message
 * or a log. The messages are about the service, never about the person.
 */
import type { AuthFailure } from "@algorik/auth";

export type AuthResponse<T extends object> =
  | ({ readonly ok: true } & T)
  | { readonly ok: false; readonly failure: AuthFailure };

/**
 * The CSRF token, held in module scope rather than component state because the
 * cookie it pairs with is browser-wide: two forms on one page must not race
 * each other into holding tokens for different cookies.
 */
let csrfToken: string | null = null;

async function fetchCsrfToken(): Promise<string> {
  const response = await fetch("/api/auth/csrf", { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`csrf endpoint answered ${response.status}`);
  }
  const body: unknown = await response.json();
  const token = (body as { token?: unknown }).token;
  if (typeof token !== "string" || token.length === 0) {
    throw new Error("csrf endpoint answered without a token");
  }
  return token;
}

/**
 * Called once per page mount, per the contract: the GET also (re)sets the CSRF
 * cookie, so a stale module-scope token from a previous page is overwritten
 * rather than trusted. A failure here is deliberately swallowed — `postAuth`
 * retries the fetch itself and is the place that can put the refusal in front
 * of the user, next to the form they were filling in.
 */
export async function primeCsrf(): Promise<void> {
  try {
    csrfToken = await fetchCsrfToken();
  } catch {
    csrfToken = null;
  }
}

function unreachable(): { ok: false; failure: AuthFailure } {
  return {
    ok: false,
    failure: {
      code: "provider_unavailable",
      message:
        "The identity service could not be reached. Check your connection and try again in a moment; nothing you typed was sent.",
    },
  };
}

function garbled(status: number): { ok: false; failure: AuthFailure } {
  return {
    ok: false,
    failure: {
      code: "provider_unavailable",
      message: `The identity service answered in a shape this page does not understand (HTTP ${status}). Try again; if it repeats, tell the operator who issued your account.`,
    },
  };
}

function isFailureShaped(value: unknown): value is { ok: false; failure: AuthFailure } {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { ok?: unknown; failure?: unknown };
  if (candidate.ok !== false) return false;
  const failure = candidate.failure as { code?: unknown; message?: unknown } | undefined;
  return typeof failure?.code === "string" && typeof failure?.message === "string";
}

/**
 * POST one auth endpoint, returning either the contract's success body or an
 * `AuthFailure` the page can render verbatim.
 *
 * Refusals come back as values, never as thrown exceptions: a wrong password
 * is an expected answer, and a page that catches it in a `try` block ends up
 * with a handler that also swallows the genuinely broken cases.
 */
export async function postAuth<T extends object>(
  path: string,
  body: unknown,
): Promise<AuthResponse<T>> {
  let token = csrfToken;
  if (!token) {
    try {
      token = await fetchCsrfToken();
      csrfToken = token;
    } catch {
      return unreachable();
    }
  }

  let response: Response;
  try {
    response = await fetch(path, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-algorik-csrf": token,
      },
      body: JSON.stringify(body),
    });
  } catch {
    return unreachable();
  }

  // Sign-out answers 204 by contract; there is no body to parse.
  if (response.status === 204) {
    return { ok: true } as { ok: true } & T;
  }

  let parsed: unknown;
  try {
    parsed = await response.json();
  } catch {
    return garbled(response.status);
  }

  if (typeof parsed === "object" && parsed !== null && (parsed as { ok?: unknown }).ok === true) {
    // The success fields (`next?`, `devCode?`) are optional in the contract,
    // so absence is already representable; re-validating each endpoint's
    // shape here would duplicate the contract without a refusal we could act on.
    return parsed as { ok: true } & T;
  }
  if (isFailureShaped(parsed)) {
    return parsed;
  }
  return garbled(response.status);
}
