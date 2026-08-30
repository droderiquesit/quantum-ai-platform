"use client";

import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { safeRedirect } from "@algorik/auth";
import type { AuthFailure } from "@algorik/auth";
import { email as emailRule, errorFor, required, validate } from "@algorik/validation";
import type { Validated } from "@algorik/validation";
import { postAuth, primeCsrf } from "../_lib/api";
import { AuthHeading, Notice, SubmitButton, SummaryError, TextField } from "../_lib/forms";

/**
 * Sign in.
 *
 * Two exits are not the happy path and both are handled before the summary
 * error is: an unverified address goes to the verification page carrying the
 * address (finishing verification is the fix, not retyping the password), and
 * the post-success destination passes through `safeRedirect` because a `next`
 * parameter honoured verbatim is an open redirect wearing our sign-in page.
 */
export default function SignInPage() {
  return (
    <Suspense fallback={<p className="text-[12px] text-[color:var(--color-ink-dim)]">Loading…</p>}>
      <SignInForm />
    </Suspense>
  );
}

function SignInForm() {
  const router = useRouter();
  const params = useSearchParams();
  const [emailValue, setEmailValue] = useState("");
  const [passwordValue, setPasswordValue] = useState("");
  const [checked, setChecked] = useState<Validated<Record<string, unknown>> | null>(null);
  const [failure, setFailure] = useState<AuthFailure | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // Derived during render, not stored: verify-email and reset-password arrive
  // here with a flag in the URL, and state would let the notice outlive it.
  const notice =
    params.get("verified") === "1"
      ? "Email verified. Sign in to continue."
      : params.get("reset") === "1"
        ? "Password reset. Sign in with your new password."
        : null;

  useEffect(() => {
    void primeCsrf();
  }, []);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedEmail = emailValue.trim();
    const result = validate(
      { email: trimmedEmail, password: passwordValue },
      {
        email: [required("Enter the email address on the account."), emailRule()],
        password: [required("Enter your password.")],
      },
    );
    setChecked(result);
    setFailure(null);
    if (!result.ok) return;

    setSubmitting(true);
    const response = await postAuth<{ next?: "verify-email" }>("/api/auth/sign-in", {
      email: trimmedEmail,
      password: passwordValue,
    });

    if (response.ok) {
      if (response.next === "verify-email") {
        router.push(`/verify-email?email=${encodeURIComponent(trimmedEmail)}`);
        return;
      }
      // A full navigation, not a client transition: the session cookie was
      // just set and every layout above this one must re-evaluate with it.
      window.location.assign(safeRedirect(new URLSearchParams(location.search).get("next"), "/"));
      return;
    }

    if (response.failure.code === "email_unverified" || response.failure.next === "verify-email") {
      router.push(`/verify-email?email=${encodeURIComponent(trimmedEmail)}`);
      return;
    }

    setSubmitting(false);
    setFailure(response.failure);
  }

  const fieldError = (field: string) => (checked ? errorFor(checked, field) : undefined);

  return (
    <div className="flex flex-col gap-4">
      <AuthHeading title="Sign in">Research console access. Paper trading only.</AuthHeading>

      {notice ? <Notice>{notice}</Notice> : null}
      {failure ? (
        <SummaryError
          message={failure.message}
          hint={
            failure.code === "account_locked" ? (
              <Link href="/account-locked" className="underline">
                Why accounts lock, and what to do
              </Link>
            ) : undefined
          }
        />
      ) : checked && !checked.ok ? (
        <SummaryError message="Nothing was sent. Fix the fields marked below." />
      ) : null}

      <form onSubmit={onSubmit} noValidate className="flex flex-col gap-3">
        <TextField
          id="email"
          label="Email"
          type="email"
          value={emailValue}
          onChange={setEmailValue}
          autoComplete="email"
          inputMode="email"
          testid="auth-email"
          error={fieldError("email")}
        />
        <TextField
          id="password"
          label="Password"
          type="password"
          value={passwordValue}
          onChange={setPasswordValue}
          autoComplete="current-password"
          testid="auth-password"
          error={fieldError("password")}
        />
        <SubmitButton busy={submitting} busyLabel="Signing in…">
          Sign in
        </SubmitButton>
      </form>

      <div>
        {/* Present but disabled: hiding it would make the deployment look
            incapable of Google identity, faking it would be worse. The caption
            says exactly which state this is. */}
        <button type="button" className="btn w-full" disabled>
          Continue with Google
        </button>
        <p className="mt-1 text-[11px] leading-snug text-[color:var(--color-ink-faint)]">
          Available once Google identity is configured for this deployment.
        </p>
      </div>

      <nav className="flex justify-between text-[12px] text-[color:var(--color-ink-dim)]">
        <Link href="/sign-up" className="hover:text-[color:var(--color-ink)]">
          Create account
        </Link>
        <Link href="/forgot-password" className="hover:text-[color:var(--color-ink)]">
          Forgot password
        </Link>
      </nav>
    </div>
  );
}
