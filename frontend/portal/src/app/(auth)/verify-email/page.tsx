"use client";

import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect, useState, useSyncExternalStore } from "react";
import type { FormEvent } from "react";
import type { AuthFailure } from "@algorik/auth";
import type { Rule } from "@algorik/validation";
import { errorFor, required, validate } from "@algorik/validation";
import type { Validated } from "@algorik/validation";
import { postAuth, primeCsrf } from "../_lib/api";
import {
  AuthHeading,
  DevCodeBlock,
  Notice,
  SubmitButton,
  SummaryError,
  TextField,
} from "../_lib/forms";

// Exactly six digits, by contract. Catching a typo here saves a round trip
// that would burn one of the endpoint's rate-limit slots on a known-bad code.
const sixDigits: Rule<string> = (value, field) =>
  /^\d{6}$/.test(value.trim()) ? null : { field, message: "Enter the 6-digit code from the message." };

/**
 * The development code sign-up parked in sessionStorage, read as an external
 * store rather than effect-plus-state: the value exists before React does, and
 * `useSyncExternalStore` gives the server render a `null` snapshot without a
 * post-hydration setState cascade. Parked in storage, not the URL, so the code
 * cannot leak into history or referrer headers.
 */
function subscribeToStorage(onChange: () => void): () => void {
  window.addEventListener("storage", onChange);
  return () => window.removeEventListener("storage", onChange);
}

function readParkedDevCode(): string | null {
  try {
    return sessionStorage.getItem("algorik.dev-code");
  } catch {
    // Storage denied costs only convenience: the resend button fetches a fresh code.
    return null;
  }
}

/**
 * `useSearchParams` must sit under a Suspense boundary or the whole route
 * falls out of static prerendering; the page shell wraps, the form reads.
 */
export default function VerifyEmailPage() {
  return (
    <Suspense fallback={<p className="text-[12px] text-[color:var(--color-ink-dim)]">Loading…</p>}>
      <VerifyEmailForm />
    </Suspense>
  );
}

function VerifyEmailForm() {
  const router = useRouter();
  const emailParam = useSearchParams().get("email");
  const [code, setCode] = useState("");
  const [checked, setChecked] = useState<Validated<Record<string, unknown>> | null>(null);
  const [failure, setFailure] = useState<AuthFailure | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [resending, setResending] = useState(false);
  const [resendNote, setResendNote] = useState<string | null>(null);
  const [resentCode, setResentCode] = useState<string | null>(null);
  const parkedCode = useSyncExternalStore(subscribeToStorage, readParkedDevCode, () => null);
  // A resend supersedes the parked code: the endpoint invalidates earlier codes.
  const devCode = resentCode ?? parkedCode;

  useEffect(() => {
    void primeCsrf();
  }, []);

  if (!emailParam) {
    return (
      <div className="flex flex-col gap-4">
        <AuthHeading title="Verify email">
          This page verifies one specific address, and none was given.
        </AuthHeading>
        <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
          Open it from{" "}
          <Link href="/sign-up" className="underline">
            sign-up
          </Link>{" "}
          or{" "}
          <Link href="/sign-in" className="underline">
            sign-in
          </Link>
          , which pass the address along.
        </p>
      </div>
    );
  }

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedCode = code.trim();
    const result = validate(
      { code: trimmedCode },
      { code: [required("Enter the verification code."), sixDigits] },
    );
    setChecked(result);
    setFailure(null);
    if (!result.ok) return;

    setSubmitting(true);
    const response = await postAuth<Record<never, never>>("/api/auth/verify-email", {
      email: emailParam,
      code: trimmedCode,
    });

    if (response.ok) {
      try {
        // The parked code is spent; leaving it would show a stale code to the
        // next journey that lands here.
        sessionStorage.removeItem("algorik.dev-code");
      } catch {
        // Storage denied: nothing was stored either.
      }
      router.push("/sign-in?verified=1");
      return;
    }

    setSubmitting(false);
    setFailure(response.failure);
  }

  async function onResend() {
    setResending(true);
    setResendNote(null);
    setFailure(null);
    const response = await postAuth<{ devCode?: string }>("/api/auth/resend-verification", {
      email: emailParam,
    });
    setResending(false);
    if (response.ok) {
      setResendNote("A new code has been issued. The previous one no longer applies.");
      if (response.devCode) setResentCode(response.devCode);
    } else {
      setFailure(response.failure);
    }
  }

  const fieldError = (field: string) => (checked ? errorFor(checked, field) : undefined);

  return (
    <div className="flex flex-col gap-4">
      <AuthHeading title="Verify email">
        A verification code was issued for <strong>{emailParam}</strong>. Enter it to finish.
      </AuthHeading>

      {devCode ? <DevCodeBlock code={devCode} /> : null}
      {resendNote ? <Notice>{resendNote}</Notice> : null}
      {failure ? (
        <SummaryError message={failure.message} />
      ) : checked && !checked.ok ? (
        <SummaryError message="Nothing was sent. Fix the field marked below." />
      ) : null}

      <form onSubmit={onSubmit} noValidate className="flex flex-col gap-3">
        <TextField
          id="code"
          label="Verification code"
          value={code}
          onChange={setCode}
          autoComplete="one-time-code"
          inputMode="numeric"
          maxLength={6}
          testid="auth-code"
          error={fieldError("code")}
        />
        <SubmitButton busy={submitting} busyLabel="Verifying…">
          Verify
        </SubmitButton>
      </form>

      <div className="flex items-center justify-between">
        <button type="button" className="btn" data-variant="ghost" onClick={onResend} disabled={resending}>
          {resending ? "Sending…" : "Resend code"}
        </button>
        <Link href="/sign-in" className="text-[12px] text-[color:var(--color-ink-dim)] hover:text-[color:var(--color-ink)]">
          Back to sign in
        </Link>
      </div>
    </div>
  );
}
