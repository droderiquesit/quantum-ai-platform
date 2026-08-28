"use client";

import { useCallback, useMemo, useState } from "react";
import { usePlatform } from "@/components/chrome/PlatformProvider";
import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { MissingEndpointBlock, StateBlock } from "@/components/data/States";
import { describeOutcome, platform, type ApiOutcome, type PaperOrderRequest } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Risk } from "@/lib/api/types";
import { formatClock } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

const DECIMAL = /^\d+(\.\d{1,8})?$/;
const INSTRUMENT = /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,31}$/;

interface Ticket {
  instrument: string;
  side: "buy" | "sell";
  quantity: string;
  orderType: "market" | "limit";
  limitPrice: string;
  timeInForce: "day" | "ioc" | "gtc";
  venue: string;
  reason: string;
}

const BLANK: Ticket = {
  instrument: "",
  side: "buy",
  quantity: "",
  orderType: "limit",
  limitPrice: "",
  timeInForce: "day",
  venue: "simulated",
  reason: "",
};

type Errors = Partial<Record<keyof Ticket, string>>;

function validate(ticket: Ticket): Errors {
  const errors: Errors = {};
  if (!INSTRUMENT.test(ticket.instrument.trim())) {
    errors.instrument = "An instrument identifier is required (letters, digits, . _ : / -).";
  }
  if (!DECIMAL.test(ticket.quantity.trim())) {
    errors.quantity = "Quantity must be a positive decimal, at most eight decimal places.";
  } else if (Number(ticket.quantity) <= 0) {
    errors.quantity = "Quantity must be greater than zero.";
  }
  if (ticket.orderType === "limit") {
    if (!DECIMAL.test(ticket.limitPrice.trim())) {
      errors.limitPrice = "A limit order needs a positive decimal limit price.";
    } else if (Number(ticket.limitPrice) <= 0) {
      errors.limitPrice = "The limit price must be greater than zero.";
    }
  }
  if (ticket.venue.trim().length === 0) {
    errors.venue = "Name the venue this simulated order is routed to.";
  }
  if (ticket.reason.trim().length < 8) {
    errors.reason = "Record why this order is being staged (at least eight characters).";
  }
  return errors;
}

function toRequest(ticket: Ticket): PaperOrderRequest {
  return {
    instrument: ticket.instrument.trim(),
    side: ticket.side,
    quantity: ticket.quantity.trim(),
    order_type: ticket.orderType,
    limit_price: ticket.orderType === "limit" ? ticket.limitPrice.trim() : null,
    time_in_force: ticket.timeInForce,
    venue: ticket.venue.trim(),
    paper: true,
    reason: ticket.reason.trim(),
  };
}

/**
 * Stage a simulated order, check it against the platform's live state, and
 * submit it for real.
 *
 * The platform serves no write path for orders, so the submission is expected
 * to come back as a missing route. It is still issued rather than blocked from
 * a hard-coded list, and whatever the platform actually answers is shown: the
 * day the route exists, this page starts working without a change here.
 */
export default function PaperOrderEntry() {
  const { health, status, halted } = usePlatform();
  const risk = useResource<Risk>(platform.risk, {
    key: "order-entry-risk",
    label: "GET /risk",
    intervalMs: 15_000,
  });

  const [ticket, setTicket] = useState<Ticket>(BLANK);
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ at: number; outcome: ApiOutcome<unknown> } | null>(null);

  const errors = useMemo(() => validate(ticket), [ticket]);
  const valid = Object.keys(errors).length === 0;
  const request = useMemo(() => toRequest(ticket), [ticket]);

  const blockers = useMemo(() => {
    const found: string[] = [];
    if (halted === true) found.push("The platform is halted; no order would be accepted.");
    if (risk.data?.kill_switch.halted) {
      found.push(
        `The kill switch is tripped${
          risk.data.kill_switch.reason ? `: ${risk.data.kill_switch.reason}` : "."
        }`,
      );
    }
    if (health.data?.live_capable) {
      found.push(
        "The platform reports live_capable = true. This console still sends paper: true on every ticket.",
      );
    }
    if ((health.data?.reconciliation_breaks ?? 0) > 0) {
      found.push(
        `${health.data?.reconciliation_breaks} reconciliation break(s) are open; the book disagrees with a venue.`,
      );
    }
    return found;
  }, [halted, risk.data, health.data]);

  const set = useCallback(<K extends keyof Ticket>(key: K, value: Ticket[K]) => {
    setTicket((previous) => ({ ...previous, [key]: value }));
  }, []);

  const submit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      setTouched(true);
      if (!valid) return;
      setBusy(true);
      try {
        const response = await platform.submitPaperOrder(request);
        setResult({ at: response.receivedAt, outcome: response.outcome });
      } finally {
        setBusy(false);
      }
    },
    [valid, request],
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Preflight"
          meta={<Freshness resource={health} name="platform health" />}
          actions={
            <>
              <StatusChip
                tone={halted === null ? "neutral" : halted ? "bad" : "ok"}
                label={halted === null ? "halt unknown" : halted ? "halted" : "running"}
              />
              {status.data ? <Chip tone="info">autonomy {status.data.autonomy}</Chip> : null}
            </>
          }
        />
        <PanelBody>
          {blockers.length === 0 ? (
            <p className="text-[12px] text-[color:var(--color-ink-dim)]">
              Nothing in the platform&rsquo;s current state blocks staging a ticket. Every order sent
              from this page carries <code className="num">paper: true</code>.
            </p>
          ) : (
            <ul className="flex flex-col gap-1.5" role="alert">
              {blockers.map((blocker) => (
                <li key={blocker} className="flex items-start gap-2 text-[12px]">
                  <span className="mt-[3px] block h-[6px] w-[6px] shrink-0 bg-[color:var(--color-warn)]" />
                  <span className="text-[color:var(--color-ink-dim)]">{blocker}</span>
                </li>
              ))}
            </ul>
          )}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Order write path" actions={<Chip tone="warn">no endpoint</Chip>} />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["submitPaperOrder"]!} />
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_minmax(0,420px)]">
        <Panel>
          <PanelHead title="Ticket" />
          <PanelBody>
            <form onSubmit={submit} noValidate className="flex flex-col gap-3">
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <Field
                  id="instrument"
                  label="Instrument"
                  error={touched ? errors.instrument : undefined}
                >
                  <input
                    id="instrument"
                    className="input"
                    value={ticket.instrument}
                    onChange={(event) => set("instrument", event.target.value)}
                    placeholder="e.g. EURUSD or BTC-USD"
                    autoComplete="off"
                    aria-invalid={touched && errors.instrument ? true : undefined}
                    aria-describedby={errors.instrument ? "instrument-error" : undefined}
                  />
                </Field>

                <div>
                  <span className="field-label" id="side-label">
                    Side
                  </span>
                  <div className="seg" role="group" aria-labelledby="side-label">
                    {(["buy", "sell"] as const).map((option) => (
                      <button
                        key={option}
                        type="button"
                        aria-pressed={ticket.side === option}
                        onClick={() => set("side", option)}
                      >
                        {option}
                      </button>
                    ))}
                  </div>
                </div>

                <Field id="quantity" label="Quantity" error={touched ? errors.quantity : undefined}>
                  <input
                    id="quantity"
                    className="input"
                    value={ticket.quantity}
                    onChange={(event) => set("quantity", event.target.value)}
                    placeholder="0.00000000"
                    inputMode="decimal"
                    autoComplete="off"
                    aria-invalid={touched && errors.quantity ? true : undefined}
                    aria-describedby={errors.quantity ? "quantity-error" : undefined}
                  />
                </Field>

                <div>
                  <span className="field-label" id="type-label">
                    Order type
                  </span>
                  <div className="seg" role="group" aria-labelledby="type-label">
                    {(["limit", "market"] as const).map((option) => (
                      <button
                        key={option}
                        type="button"
                        aria-pressed={ticket.orderType === option}
                        onClick={() => set("orderType", option)}
                      >
                        {option}
                      </button>
                    ))}
                  </div>
                </div>

                <Field
                  id="limitPrice"
                  label={ticket.orderType === "limit" ? "Limit price" : "Limit price (not used)"}
                  error={touched ? errors.limitPrice : undefined}
                >
                  <input
                    id="limitPrice"
                    className="input"
                    value={ticket.limitPrice}
                    onChange={(event) => set("limitPrice", event.target.value)}
                    placeholder={ticket.orderType === "limit" ? "0.00" : "—"}
                    inputMode="decimal"
                    autoComplete="off"
                    disabled={ticket.orderType !== "limit"}
                    aria-invalid={touched && errors.limitPrice ? true : undefined}
                    aria-describedby={errors.limitPrice ? "limitPrice-error" : undefined}
                  />
                </Field>

                <Field id="timeInForce" label="Time in force">
                  <select
                    id="timeInForce"
                    className="select"
                    value={ticket.timeInForce}
                    onChange={(event) =>
                      set("timeInForce", event.target.value as Ticket["timeInForce"])
                    }
                  >
                    <option value="day">day</option>
                    <option value="ioc">immediate or cancel</option>
                    <option value="gtc">good till cancelled</option>
                  </select>
                </Field>

                <Field id="venue" label="Venue" error={touched ? errors.venue : undefined}>
                  <input
                    id="venue"
                    className="input"
                    value={ticket.venue}
                    onChange={(event) => set("venue", event.target.value)}
                    autoComplete="off"
                    aria-invalid={touched && errors.venue ? true : undefined}
                    aria-describedby={errors.venue ? "venue-error" : undefined}
                  />
                </Field>
              </div>

              <Field id="reason" label="Reason (recorded with the ticket)" error={touched ? errors.reason : undefined}>
                <textarea
                  id="reason"
                  className="textarea"
                  rows={2}
                  value={ticket.reason}
                  onChange={(event) => set("reason", event.target.value)}
                  placeholder="why this order is being staged"
                  aria-invalid={touched && errors.reason ? true : undefined}
                  aria-describedby={errors.reason ? "reason-error" : undefined}
                />
              </Field>

              <div className="flex flex-wrap items-center gap-2 border-t border-[color:var(--color-line)] pt-3">
                <span className="chip" data-tone="warn">
                  paper: true
                </span>
                <span className="text-[11.5px] text-[color:var(--color-ink-faint)]">
                  Submitting sends <code className="num">POST /api/v1/orders</code> to the platform.
                </span>
                <div className="ml-auto flex gap-2">
                  <button
                    type="button"
                    className="btn"
                    data-variant="ghost"
                    onClick={() => {
                      setTicket(BLANK);
                      setTouched(false);
                      setResult(null);
                    }}
                  >
                    Reset
                  </button>
                  <button
                    type="submit"
                    className="btn"
                    data-variant="primary"
                    disabled={busy}
                    data-testid="submit-paper-order"
                  >
                    {busy ? "Submitting…" : "Submit paper order"}
                  </button>
                </div>
              </div>

              {touched && !valid ? (
                <p className="text-[11.5px] text-[color:var(--color-down)]" role="alert">
                  {Object.keys(errors).length} field(s) need attention before this ticket can be
                  submitted.
                </p>
              ) : null}
            </form>
          </PanelBody>
        </Panel>

        <div className="flex flex-col gap-3">
          <Panel>
            <PanelHead
              title="Wire body"
              meta={
                <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                  exactly what would be sent
                </span>
              }
            />
            <PanelBody>
              <pre className="num max-h-[280px] overflow-auto whitespace-pre-wrap break-all border border-[color:var(--color-line)] bg-[color:var(--color-sunken)] p-2 text-[11px] text-[color:var(--color-ink-dim)]">
                {JSON.stringify(request, null, 2)}
              </pre>
            </PanelBody>
          </Panel>

          <Panel>
            <PanelHead
              title="Submission result"
              meta={
                result ? (
                  <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                    {formatClock(result.at)}
                  </span>
                ) : null
              }
            />
            <PanelBody>
              {result === null ? (
                <p className="text-[12px] text-[color:var(--color-ink-faint)]">
                  Nothing has been submitted from this console in this session.
                </p>
              ) : result.outcome.kind === "ok" ? (
                <StateBlock
                  tone="info"
                  label="accepted"
                  headline="The platform accepted the paper order."
                  compact
                >
                  <pre className="num mt-1 max-h-[200px] overflow-auto whitespace-pre-wrap break-all">
                    {JSON.stringify(result.outcome.data, null, 2)}
                  </pre>
                </StateBlock>
              ) : result.outcome.kind === "missing" ? (
                <StateBlock
                  tone="warn"
                  label="endpoint missing"
                  headline={`Not yet available — POST /api/v1/orders answered ${result.outcome.status}.`}
                  compact
                >
                  <p>{result.outcome.detail}</p>
                  <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
                    The ticket above was sent as written. Nothing was staged, queued or retried, and
                    nothing was recorded locally in its place.
                  </p>
                </StateBlock>
              ) : (
                <StateBlock
                  tone="bad"
                  label={result.outcome.kind}
                  headline="The platform refused the submission."
                  compact
                >
                  <p>{describeOutcome(result.outcome)}</p>
                </StateBlock>
              )}
            </PanelBody>
          </Panel>
        </div>
      </div>
    </div>
  );
}

function Field({
  id,
  label,
  error,
  children,
}: {
  id: string;
  label: string;
  error?: string | undefined;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label className="field-label" htmlFor={id}>
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${id}-error`} className="mt-1 text-[11px] text-[color:var(--color-down)]">
          {error}
        </p>
      ) : null}
    </div>
  );
}
