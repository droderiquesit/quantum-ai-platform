"use client";

import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect, useState } from "react";
import type { FormEvent } from "react";
import type { AuthFailure } from "@algorik/auth";
import type { Rule } from "@algorik/validation";
import {
  email as emailRule,
  errorFor,
  matches,
  password as passwordRule,
  required,
  validate,
} from "@algorik/validation";
import type { Validated } from "@algorik/validation";
import { postAuth, primeCsrf } from "../_lib/api";
import { AuthHeading, SubmitButton, SummaryError, TextField } from "../_lib/forms";

// Exactly six digits, by contract — the same shape the verify page enforces,
// and for the same reason: a typo should not cost a rate-limit slot.
const sixDigits: Rule<string> = (value, field) =>
  /^\d{6}$/.test(value.trim()) ? null : { field, message: "Enter the 6-digit code from the message." };

/**
 * `useSearchParams` must sit under a Suspense boundary or the whole route
 * falls out of static prerendering; the page shell wraps, the form reads.
 */
export default function ResetPasswordPage() {
  return (
    <Suspense fallback={<p className="text-[12px] text-[color:var(--color-ink-dim)]">Loading…</p>}>
      <ResetPasswordForm />
    </Suspense>
  );
}

/**
 * The email is prefilled from the link but stays editable: the person may have
 * requested the code for one address and opened this page from a bookmark, and
 * a hidden field would leave them resetting the wrong account with no way to
 * see it.
 */
function ResetPasswordForm() {
  const router = useRouter();
  const emailParam = useSearchParams().get("email");
  const [emailValue, setEmailValue] = useState(emailParam ?? "");
  const [code, setCode] = useState("");
  const [passwordValue, setPasswordValue] = useState("");
  const [confirmValue, setConfirmValue] = useState("");
  const [checked, setChecked] = useState<Validated<Record<string, unknown>> | null>(null);
  const [failure, setFailure] = useState<AuthFailure | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    void primeCsrf();
  }, []);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedEmail = emailValue.trim();
    const trimmedCode = code.trim();
    const result = validate(
      { email: trimmedEmail, code: trimmedCode, password: passwordValue, confirm: confirmValue },
      {
        email: [required("Enter the email address on the account."), emailRule()],
        code: [required("Enter the reset code."), sixDigits],
        password: [required("Choose a new password."), passwordRule()],
        confirm: [required("Repeat the new password."), matches(passwordValue, "the password above")],
      },
    );
    setChecked(result);
    setFailure(null);
    if (!result.ok) return;

    setSubmitting(true);
    const response = await postAuth<Record<never, never>>("/api/auth/reset-password", {
      email: trimmedEmail,
      code: trimmedCode,
      password: passwordValue,
    });

    if (response.ok) {
      router.push("/sign-in?reset=1");
      return;
    }

    setSubmitting(false);
    setFailure(response.failure);
  }

  const fieldError = (field: string) => (checked ? errorFor(checked, field) : undefined);

  return (
    <div className="flex flex-col gap-4">
      <AuthHeading title="Reset password">
        Enter the code that was issued and choose a new password.
      </AuthHeading>

      {failure ? (
        <SummaryError
          message={failure.message}
          hint={
            <Link href="/forgot-password" className="underline">
              Request a new code
            </Link>
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
          id="code"
          label="Reset code"
          value={code}
          onChange={setCode}
          autoComplete="one-time-code"
          inputMode="numeric"
          maxLength={6}
          testid="auth-code"
          error={fieldError("code")}
        />
        <TextField
          id="password"
          label="New password"
          type="password"
          value={passwordValue}
          onChange={setPasswordValue}
          autoComplete="new-password"
          testid="auth-password"
          error={fieldError("password")}
        />
        <TextField
          id="confirm"
          label="Confirm new password"
          type="password"
          value={confirmValue}
          onChange={setConfirmValue}
          autoComplete="new-password"
          testid="auth-password-confirm"
          error={fieldError("confirm")}
        />
        <SubmitButton busy={submitting} busyLabel="Resetting…">
          Reset password
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
