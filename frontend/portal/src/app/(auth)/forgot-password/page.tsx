"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import type { AuthFailure } from "@algorik/auth";
import { email as emailRule, errorFor, required, validate } from "@algorik/validation";
import type { Validated } from "@algorik/validation";
import { postAuth, primeCsrf } from "../_lib/api";
import { AuthHeading, DevCodeBlock, SubmitButton, SummaryError, TextField } from "../_lib/forms";

/**
 * Request a password reset.
 *
 * The endpoint answers 200 whether or not the address has an account, and the
 * confirmation sentence is worded to match: this page must not become the
 * oracle that lets someone enumerate which addresses are registered. The only
 * failure it ever shows is the service being unreachable — never "no such
 * account".
 */
export default function ForgotPasswordPage() {
  const [emailValue, setEmailValue] = useState("");
  const [checked, setChecked] = useState<Validated<Record<string, unknown>> | null>(null);
  const [failure, setFailure] = useState<AuthFailure | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [issuedFor, setIssuedFor] = useState<string | null>(null);
  const [devCode, setDevCode] = useState<string | null>(null);

  useEffect(() => {
    void primeCsrf();
  }, []);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedEmail = emailValue.trim();
    const result = validate(
      { email: trimmedEmail },
      { email: [required("Enter the email address on the account."), emailRule()] },
    );
    setChecked(result);
    setFailure(null);
    if (!result.ok) return;

    setSubmitting(true);
    const response = await postAuth<{ devCode?: string }>("/api/auth/forgot-password", {
      email: trimmedEmail,
    });
    setSubmitting(false);

    if (response.ok) {
      setIssuedFor(trimmedEmail);
      setDevCode(response.devCode ?? null);
    } else {
      setFailure(response.failure);
    }
  }

  const fieldError = (field: string) => (checked ? errorFor(checked, field) : undefined);

  if (issuedFor) {
    return (
      <div className="flex flex-col gap-4">
        <AuthHeading title="Reset requested">
          If that address has an account, a reset code has been issued.
        </AuthHeading>
        {devCode ? <DevCodeBlock code={devCode} /> : null}
        <Link
          href={`/reset-password?email=${encodeURIComponent(issuedFor)}`}
          className="btn w-full"
          data-variant="primary"
        >
          Enter the reset code
        </Link>
        <p className="text-[12px] text-[color:var(--color-ink-dim)]">
          Remembered it after all?{" "}
          <Link href="/sign-in" className="underline hover:text-[color:var(--color-ink)]">
            Sign in
          </Link>
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <AuthHeading title="Forgot password">
        Enter the address on the account and a reset code will be issued.
      </AuthHeading>

      {failure ? (
        <SummaryError message={failure.message} />
      ) : checked && !checked.ok ? (
        <SummaryError message="Nothing was sent. Fix the field marked below." />
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
        <SubmitButton busy={submitting} busyLabel="Requesting…">
          Request reset code
        </SubmitButton>
      </form>

      <p className="text-[12px] text-[color:var(--color-ink-dim)]">
        <Link href="/sign-in" className="underline hover:text-[color:var(--color-ink)]">
          Back to sign in
        </Link>
      </p>
    </div>
  );
}
