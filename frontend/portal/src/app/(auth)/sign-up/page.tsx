"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import type { AccountType, AuthFailure } from "@algorik/auth";
import {
  accepted,
  email as emailRule,
  errorFor,
  matches,
  password as passwordRule,
  required,
  validate,
} from "@algorik/validation";
import type { Validated } from "@algorik/validation";
import { postAuth, primeCsrf } from "../_lib/api";
import {
  AuthHeading,
  CheckField,
  SelectField,
  SubmitButton,
  SummaryError,
  TextField,
} from "../_lib/forms";

const ACCOUNT_TYPES: readonly { readonly value: AccountType; readonly label: string }[] = [
  { value: "individual", label: "Individual" },
  { value: "institutional", label: "Institutional" },
  { value: "partner", label: "Partner" },
  { value: "developer", label: "Developer" },
];

/**
 * Create an account.
 *
 * The three agreement boxes are separate and each names its document, because
 * one "I accept everything" checkbox is acceptance of nothing in particular —
 * the record needs to show which document, at which version, was in front of
 * the person. The plain-language note under them exists for the person who
 * assumed a trading platform sign-up buys trading: it grants a paper-trading
 * research account and says so before they commit.
 */
export default function SignUpPage() {
  const router = useRouter();
  const [accountType, setAccountType] = useState<AccountType>("individual");
  const [displayName, setDisplayName] = useState("");
  const [emailValue, setEmailValue] = useState("");
  const [passwordValue, setPasswordValue] = useState("");
  const [confirmValue, setConfirmValue] = useState("");
  const [terms, setTerms] = useState(false);
  const [privacy, setPrivacy] = useState(false);
  const [risk, setRisk] = useState(false);
  const [checked, setChecked] = useState<Validated<Record<string, unknown>> | null>(null);
  const [failure, setFailure] = useState<AuthFailure | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    void primeCsrf();
  }, []);

  function selectAccountType(value: string) {
    // Looked up rather than cast: a value the option list does not contain is
    // a bug in this page, and a cast would send it to the server as fact.
    const match = ACCOUNT_TYPES.find((candidate) => candidate.value === value);
    if (match) setAccountType(match.value);
  }

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedEmail = emailValue.trim();
    const result = validate(
      { email: trimmedEmail, password: passwordValue, confirm: confirmValue, terms, privacy, risk },
      {
        email: [required("Enter the email address to register."), emailRule()],
        password: [required("Choose a password."), passwordRule()],
        confirm: [required("Repeat the password."), matches(passwordValue, "the password above")],
        terms: [accepted("Accept the Terms of Service to continue.")],
        privacy: [accepted("Accept the Privacy Policy to continue.")],
        risk: [accepted("Accept the Risk Disclosures to continue.")],
      },
    );
    setChecked(result);
    setFailure(null);
    if (!result.ok) return;

    setSubmitting(true);
    const trimmedName = displayName.trim();
    const response = await postAuth<{ next: "verify-email"; devCode?: string }>(
      "/api/auth/sign-up",
      {
        email: trimmedEmail,
        password: passwordValue,
        accountType,
        // Literal `true`, not the checkbox state: `accepted()` refused any
        // unchecked box above, and the contract records acceptance, not a DOM
        // reading that could have been tampered with between then and now.
        agreements: { terms: true, privacy: true, riskDisclosure: true },
        ...(trimmedName ? { displayName: trimmedName } : {}),
      },
    );

    if (response.ok) {
      if (response.devCode) {
        try {
          sessionStorage.setItem("algorik.dev-code", response.devCode);
        } catch {
          // Storage denied costs only convenience: the verify page's resend
          // button fetches a fresh code.
        }
      }
      router.push(`/verify-email?email=${encodeURIComponent(trimmedEmail)}`);
      return;
    }

    if (response.failure.next === "verify-email") {
      router.push(`/verify-email?email=${encodeURIComponent(trimmedEmail)}`);
      return;
    }

    setSubmitting(false);
    setFailure(response.failure);
  }

  const fieldError = (field: string) => (checked ? errorFor(checked, field) : undefined);

  return (
    <div className="flex flex-col gap-4">
      <AuthHeading title="Create account">
        A paper-trading research account on Algorik.
      </AuthHeading>

      {failure ? (
        <SummaryError message={failure.message} />
      ) : checked && !checked.ok ? (
        <SummaryError message="Nothing was sent. Fix the fields marked below." />
      ) : null}

      <form onSubmit={onSubmit} noValidate className="flex flex-col gap-3">
        <SelectField
          id="account-type"
          label="Account type"
          value={accountType}
          onChange={selectAccountType}
          options={ACCOUNT_TYPES}
          testid="auth-accounttype"
        />
        <TextField
          id="display-name"
          label="Display name (optional)"
          value={displayName}
          onChange={setDisplayName}
          autoComplete="name"
        />
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
          autoComplete="new-password"
          testid="auth-password"
          error={fieldError("password")}
        />
        <TextField
          id="confirm"
          label="Confirm password"
          type="password"
          value={confirmValue}
          onChange={setConfirmValue}
          autoComplete="new-password"
          testid="auth-password-confirm"
          error={fieldError("confirm")}
        />

        <fieldset className="flex flex-col gap-2 border-0 p-0">
          <legend className="field-label">Agreements</legend>
          <CheckField
            id="agree-terms"
            checked={terms}
            onChange={setTerms}
            testid="auth-terms"
            error={fieldError("terms")}
          >
            I accept the{" "}
            <Link href="/legal/terms" className="underline">
              Terms of Service
            </Link>
            .
          </CheckField>
          <CheckField
            id="agree-privacy"
            checked={privacy}
            onChange={setPrivacy}
            testid="auth-privacy"
            error={fieldError("privacy")}
          >
            I accept the{" "}
            <Link href="/legal/privacy" className="underline">
              Privacy Policy
            </Link>
            .
          </CheckField>
          <CheckField
            id="agree-risk"
            checked={risk}
            onChange={setRisk}
            testid="auth-risk"
            error={fieldError("risk")}
          >
            I accept the{" "}
            <Link href="/legal/risk-disclosures" className="underline">
              Risk Disclosures
            </Link>
            .
          </CheckField>
        </fieldset>

        <p className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">
          You are creating a paper-trading research account. No live trading, wallet movement, or
          production API access is granted by sign-up.
        </p>

        <SubmitButton busy={submitting} busyLabel="Creating account…">
          Create account
        </SubmitButton>
      </form>

      <p className="text-[12px] text-[color:var(--color-ink-dim)]">
        Already have an account?{" "}
        <Link href="/sign-in" className="underline hover:text-[color:var(--color-ink)]">
          Sign in
        </Link>
      </p>
    </div>
  );
}
