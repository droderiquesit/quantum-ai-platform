"use client";

/**
 * The one form idiom every auth page wears.
 *
 * Shared so a refusal reads the same on every page: the field that failed says
 * why beneath itself, and the one `role="alert"` lives on the summary — a
 * screen reader announcing five inline errors and a summary at once turns a
 * fixable form into noise, so only the summary interrupts.
 */
import type { ReactNode } from "react";

interface TextFieldProps {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly type?: "text" | "email" | "password";
  readonly autoComplete?: string;
  readonly testid?: string;
  readonly error?: string;
  readonly inputMode?: "numeric" | "email" | "text";
  readonly maxLength?: number;
  readonly placeholder?: string;
}

export function TextField({
  id,
  label,
  value,
  onChange,
  type = "text",
  autoComplete,
  testid,
  error,
  inputMode,
  maxLength,
  placeholder,
}: TextFieldProps) {
  const errorId = `${id}-error`;
  return (
    <div>
      <label className="field-label" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        className="input"
        type={type}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        autoComplete={autoComplete}
        inputMode={inputMode}
        maxLength={maxLength}
        placeholder={placeholder}
        data-testid={testid}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? errorId : undefined}
      />
      {error ? (
        <p id={errorId} className="mt-1 text-[12px] leading-snug text-[color:var(--color-critical)]">
          {error}
        </p>
      ) : null}
    </div>
  );
}

interface SelectFieldProps {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly options: readonly { readonly value: string; readonly label: string }[];
  readonly testid?: string;
  readonly error?: string;
}

export function SelectField({ id, label, value, onChange, options, testid, error }: SelectFieldProps) {
  const errorId = `${id}-error`;
  return (
    <div>
      <label className="field-label" htmlFor={id}>
        {label}
      </label>
      <select
        id={id}
        className="select"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        data-testid={testid}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? errorId : undefined}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {error ? (
        <p id={errorId} className="mt-1 text-[12px] leading-snug text-[color:var(--color-critical)]">
          {error}
        </p>
      ) : null}
    </div>
  );
}

interface CheckFieldProps {
  readonly id: string;
  readonly checked: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly testid?: string;
  readonly error?: string;
  readonly children: ReactNode;
}

export function CheckField({ id, checked, onChange, testid, error, children }: CheckFieldProps) {
  const errorId = `${id}-error`;
  return (
    <div>
      <div className="flex items-start gap-2">
        <input
          type="checkbox"
          id={id}
          checked={checked}
          onChange={(event) => onChange(event.target.checked)}
          data-testid={testid}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
          className="mt-[2px] shrink-0"
          style={{ accentColor: "var(--color-brand-primary)" }}
        />
        <label htmlFor={id} className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
          {children}
        </label>
      </div>
      {error ? (
        <p id={errorId} className="mt-1 text-[12px] leading-snug text-[color:var(--color-critical)]">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/**
 * The summary refusal. Callers render it only when there is something to say,
 * so its insertion is what triggers the `role="alert"` announcement.
 */
export function SummaryError({ message, hint }: { readonly message: string; readonly hint?: ReactNode }) {
  return (
    <div
      role="alert"
      data-testid="auth-error"
      className="border px-3 py-2 text-[12px] leading-snug text-[color:var(--color-critical)]"
      style={{ borderColor: "color-mix(in srgb, var(--color-critical) 45%, transparent)" }}
    >
      <p>{message}</p>
      {hint ? <p className="mt-1">{hint}</p> : null}
    </div>
  );
}

/** A quiet good-news line — `role="status"` so it is read without interrupting. */
export function Notice({ children }: { readonly children: ReactNode }) {
  return (
    <p
      role="status"
      className="border px-3 py-2 text-[12px] leading-snug text-[color:var(--color-success)]"
      style={{ borderColor: "color-mix(in srgb, var(--color-success) 45%, transparent)" }}
    >
      {children}
    </p>
  );
}

/**
 * Disabled while the request is in flight, with the label saying so: the
 * double-submit this prevents is not hypothetical on a sign-up endpoint, and a
 * button that merely greys out leaves the person wondering whether it took.
 */
export function SubmitButton({
  busy,
  busyLabel,
  children,
}: {
  readonly busy: boolean;
  readonly busyLabel: string;
  readonly children: ReactNode;
}) {
  return (
    <button type="submit" className="btn w-full" data-variant="primary" data-testid="auth-submit" disabled={busy}>
      {busy ? busyLabel : children}
    </button>
  );
}

/**
 * The development-identity code, unmistakably marked as such. The chip and the
 * sentence exist so nobody screenshots this block into a bug report believing
 * production emails codes onto the page — the code appears here only because
 * no email service exists locally, and the block says exactly that.
 */
export function DevCodeBlock({ code }: { readonly code: string }) {
  return (
    <div
      className="flex flex-col gap-2 border bg-[color:var(--color-sunken)] p-3"
      style={{ borderColor: "color-mix(in srgb, var(--color-warning) 45%, transparent)" }}
    >
      <span className="chip self-start" style={{ color: "var(--color-warning)", borderColor: "color-mix(in srgb, var(--color-warning) 45%, transparent)" }}>
        DEVELOPMENT IDENTITY
      </span>
      <p className="text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
        No email service is connected. Your verification code is shown here and would never be in
        production.
      </p>
      <code
        data-testid="dev-code"
        className="font-mono text-[22px] tracking-[0.3em] text-[color:var(--color-ink)]"
      >
        {code}
      </code>
    </div>
  );
}

/** Every page opens the same way: what this page does, in one short sentence. */
export function AuthHeading({ title, children }: { readonly title: string; readonly children?: ReactNode }) {
  return (
    <header>
      <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">{title}</h1>
      {children ? (
        <p className="mt-1 text-[12px] leading-snug text-[color:var(--color-ink-dim)]">{children}</p>
      ) : null}
    </header>
  );
}
