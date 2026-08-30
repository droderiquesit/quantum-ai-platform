/**
 * Product analytics, as an abstraction with no vendor behind it.
 *
 * The brief asks for a privacy-aware abstraction, and that is precisely what
 * this is: an interface, a no-op default, and a hard rule about what may be
 * recorded. No SDK is admitted (ADR 0013) — a third-party analytics script on
 * an authenticated trading surface can read the DOM of a page showing a book.
 *
 * The rule this file enforces, rather than documents: **an event may not carry
 * a value a person typed, a credential, an identifier of money, or a free-text
 * field.** Property values are constrained to a small type and every event is
 * screened before it is dispatched.
 */

/** Property values narrow enough that a token cannot be smuggled in one. */
export type AnalyticsValue = string | number | boolean;

export interface AnalyticsEvent {
  /** Dotted, past tense: "portfolio.viewed", "sign_up.completed". */
  readonly name: string;
  readonly properties?: Readonly<Record<string, AnalyticsValue>>;
}

export interface AnalyticsSink {
  readonly name: string;
  record(event: AnalyticsEvent): void;
}

/**
 * Property names that must never be sent, and substrings that imply them.
 *
 * Checked on the key, not the value: a screen cannot tell whether a string is
 * a password, but it can tell that somebody named the property one.
 */
const FORBIDDEN = [
  "password", "token", "secret", "credential", "authorization", "cookie",
  "email", "phone", "address", "ssn", "tax", "account_number", "iban",
  "balance", "value", "amount", "quantity", "pnl", "position", "apikey", "api_key",
] as const;

function isForbidden(key: string): boolean {
  const lowered = key.toLowerCase();
  return FORBIDDEN.some((banned) => lowered.includes(banned));
}

/** Strips forbidden properties. Returns what was dropped, so a test can see it. */
export function screen(event: AnalyticsEvent): {
  readonly safe: AnalyticsEvent;
  readonly dropped: readonly string[];
} {
  const properties = event.properties ?? {};
  const dropped = Object.keys(properties).filter(isForbidden);
  if (dropped.length === 0) return { safe: event, dropped };
  const kept: Record<string, AnalyticsValue> = {};
  for (const [key, value] of Object.entries(properties)) {
    if (!isForbidden(key)) kept[key] = value;
  }
  return { safe: { name: event.name, properties: kept }, dropped };
}

/** The default. Records nothing, which is the right behaviour with no consent. */
export const noopSink: AnalyticsSink = { name: "noop", record: () => {} };

/**
 * An analytics façade that screens every event and honours consent.
 *
 * Consent defaults to withheld. An analytics client that records until told to
 * stop has already recorded the thing it should not have.
 */
export class Analytics {
  private consented = false;

  constructor(private readonly sink: AnalyticsSink = noopSink) {}

  grantConsent(): void {
    this.consented = true;
  }

  withdrawConsent(): void {
    this.consented = false;
  }

  record(event: AnalyticsEvent): void {
    if (!this.consented) return;
    const { safe } = screen(event);
    this.sink.record(safe);
  }
}
