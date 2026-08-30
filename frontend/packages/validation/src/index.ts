/**
 * Validation, written in-tree.
 *
 * ADR 0013 refuses a schema library here: a shape mismatch fails loudly on the
 * first request, which is exactly the condition ADR 0012 requires a dependency
 * to *not* satisfy. What the surfaces actually need is a handful of field
 * rules and one composer, and that is what this is.
 *
 * Every rule returns the reason it refused, phrased for the person who typed
 * it. "Invalid input" tells a user nothing and is the reason they abandon a
 * sign-up form.
 */

export interface FieldError {
  readonly field: string;
  readonly message: string;
}

export type Validated<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly errors: readonly FieldError[] };

export type Rule<T> = (value: T, field: string) => FieldError | null;

export function required<T>(message = "This field is required."): Rule<T> {
  return (value, field) => {
    const empty =
      value === null || value === undefined || (typeof value === "string" && value.trim() === "");
    return empty ? { field, message } : null;
  };
}

/**
 * Email, checked shallowly on purpose.
 *
 * The only authority on whether an address exists is a delivered verification
 * message. A stricter regex rejects valid addresses — plus-addressing, new
 * TLDs, unicode domains — and the failure mode is a person who cannot sign up
 * and cannot find out why.
 */
export function email(message = "Enter an email address, for example name@company.com."): Rule<string> {
  return (value, field) => {
    const trimmed = value.trim();
    const shaped = trimmed.length >= 3 && /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/.test(trimmed);
    return shaped ? null : { field, message };
  };
}

/** Minimum length, stated in the message so the user is not guessing. */
export function minLength(least: number, message?: string): Rule<string> {
  return (value, field) =>
    value.length >= least
      ? null
      : { field, message: message ?? `Use at least ${least} characters.` };
}

export function maxLength(most: number, message?: string): Rule<string> {
  return (value, field) =>
    value.length <= most ? null : { field, message: message ?? `Use at most ${most} characters.` };
}

/**
 * Password strength, by length and variety rather than by a character-class
 * checklist.
 *
 * Composition rules ("one uppercase, one symbol") measurably produce weaker,
 * more predictable passwords and are what NIST stopped recommending. Length is
 * what matters, so length is what is required — and the message says so
 * instead of listing rules the user must satisfy by trial and error.
 */
export function password(message?: string): Rule<string> {
  return (value, field) => {
    if (value.length < 12) {
      return { field, message: message ?? "Use at least 12 characters. Length matters more than symbols." };
    }
    if (/^(.)\1+$/.test(value)) {
      return { field, message: "Use more than one repeated character." };
    }
    return null;
  };
}

export function matches(other: string, label: string, message?: string): Rule<string> {
  return (value, field) =>
    value === other ? null : { field, message: message ?? `This does not match ${label}.` };
}

export function accepted(message = "You must accept this to continue."): Rule<boolean> {
  return (value, field) => (value ? null : { field, message });
}

/** Runs each field's rules and collects every failure, not just the first. */
export function validate<T extends Record<string, unknown>>(
  values: T,
  rules: { readonly [K in keyof T]?: readonly Rule<T[K]>[] },
): Validated<T> {
  const errors: FieldError[] = [];
  for (const field of Object.keys(rules) as (keyof T & string)[]) {
    for (const rule of rules[field] ?? []) {
      const failure = rule(values[field], field);
      if (failure) {
        // One message per field: a field showing three complaints at once
        // makes the user fix them one reload at a time.
        errors.push(failure);
        break;
      }
    }
  }
  return errors.length === 0 ? { ok: true, value: values } : { ok: false, errors };
}

/** The message for one field, for rendering beside the input. */
export function errorFor(
  result: Validated<unknown>,
  field: string,
): string | undefined {
  if (result.ok) return undefined;
  return result.errors.find((error) => error.field === field)?.message;
}
