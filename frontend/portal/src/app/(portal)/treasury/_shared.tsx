"use client";

import type { ReactNode } from "react";
import { Chip, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { formatDecimal, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { INFLOW_ROUTES, type Capability, type ExpectedInflow } from "@/lib/hooks/useTreasury";

/**
 * What every treasury page carries, and why it is one component.
 *
 * The four pages under `/treasury` render the ledger plane and can move
 * nothing: no proposal, approval, signature or transfer control exists on any
 * of them, and `client.ts` declares no write they could call. That is a fact
 * about the console, so it is stated unconditionally, in the page and not
 * only in the chrome, on every one of them.
 *
 * Two postures sit beside that statement, both reports and neither an
 * assumption. The body's own `posture` literal — every treasury route answers
 * `"PAPER TRADING"` as its first key, and the contract says render it — is
 * shown as it came, so a body that ever said something else would be shown
 * saying it. And the platform's live capability from `GET /system/status`,
 * read exactly as the dataflow page reads it: `PAPER TRADING` when the
 * platform reports it is not live-capable, a red `LIVE-CAPABLE` alarm if it
 * ever reports otherwise.
 *
 * One component rather than four copies so that a page cannot be added to
 * this section without the declaration: the specs assert it on each page, and
 * a page that dropped this header would fail all of them.
 */
export function TreasuryHeader({
  title,
  reads,
  posture,
  meta,
}: {
  title: string;
  /** The routes this page reads, named so an operator knows where a figure came from. */
  reads: string;
  /** The body's own `posture` literal, once it has landed. */
  posture: string | null;
  meta?: ReactNode;
}) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: `treasury-status-${title.toLowerCase().replace(/\s+/g, "-")}`,
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="treasury-header">
      <PanelHead
        title={title}
        meta={meta}
        actions={
          <>
            {posture === null ? null : (
              <Chip tone={posture === "PAPER TRADING" ? "ok" : "bad"} title={`${reads}: posture`}>
                <span data-testid="treasury-body-posture">{posture}</span>
              </Chip>
            )}
            {status.data === null ? null : (
              <StatusChip
                tone={status.data.live_capable ? "bad" : "ok"}
                label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /system/status: live_capable"
              />
            )}
          </>
        }
      />
      <PanelBody>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="treasury-declaration">
          <span className="chip mr-2" data-tone="ok" data-testid="treasury-paper-label">
            PAPER TRADING
          </span>
          Nothing on this page can move capital. It reads {reads} and renders what the platform
          answered; there is no control here that proposes, approves, signs or transfers, and the
          gateway declares no write this page could call. ADR 0021 refuses the half of the treasury
          by which capital leaves the platform and ADR 0023 keeps that in force.
        </p>
      </PanelBody>
    </Panel>
  );
}

/** A `Capability` as the platform decided it, with its basis or reason. */
export function CapabilityChip({ label, capability }: { label: string; capability: Capability }) {
  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex items-center gap-1.5">
        <span className="eyebrow">{label}</span>
        <Chip tone={capability.granted ? "ok" : "warn"}>{capability.granted ? "granted" : "refused"}</Chip>
      </div>
      <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{capability.reason}</span>
    </div>
  );
}

/**
 * The withdrawal arm, rendered as refused with the platform's reason.
 *
 * The platform's `WithdrawalEntitlement` has one variant and the route says
 * `granted` is always `false`. This is still a report, not an assumption: if
 * a body ever carried `granted: true` here the page would not quietly show it
 * as refused — it would raise the contradiction as an alarm, because a
 * console that rendered the safe answer over the platform's actual answer
 * would be the console that hid the day the boundary moved.
 */
export function WithdrawalChip({ entitlement }: { entitlement: Capability }) {
  if (entitlement.granted) {
    return (
      <div className="flex flex-col gap-0.5" data-testid="withdrawal-entitlement" data-alert="true" role="alert">
        <div className="flex items-center gap-1.5">
          <span className="eyebrow">can withdraw</span>
          <Chip tone="bad">GRANTED — CONTRADICTS ADR 0021</Chip>
        </div>
        <span className="text-[11px] leading-snug text-[color:var(--color-down)]">
          The platform answered a granted withdrawal, which its own type cannot hold. Stop and
          investigate the process serving this route. Basis given: {entitlement.reason}
        </span>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-0.5" data-testid="withdrawal-entitlement">
      <div className="flex items-center gap-1.5">
        <span className="eyebrow">can withdraw</span>
        <Chip tone="bad">refused</Chip>
      </div>
      <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{entitlement.reason}</span>
    </div>
  );
}

/** A subsystem the platform said it does not hold, in its own words, under the caller's label. */
export function AbsentBlock({
  label,
  headline,
  reason,
  testId,
}: {
  label: string;
  headline: string;
  reason: string | null;
  testId?: string;
}) {
  return (
    <div data-testid={testId}>
      <StateBlock tone="warn" label={label} headline={headline}>
        <p>{reason ?? "The platform gave no reason."}</p>
      </StateBlock>
    </div>
  );
}

export function Muted({ children }: { children: ReactNode }) {
  return <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{children}</span>;
}

/** Text from the platform, rendered as it came. */
export function Quote({ children, tone }: { children: ReactNode; tone?: "warn" | "bad" }) {
  const colour =
    tone === "bad" ? "var(--color-down)" : tone === "warn" ? "var(--color-warn)" : "var(--color-ink-dim)";
  return (
    <p className="text-[11.5px] leading-relaxed" style={{ color: colour }}>
      {children}
    </p>
  );
}

// --- expected inflows and the held bucket (ADR 0085) --------------------------------

/**
 * The declared inflows of one book, each by its reference, amount and the
 * instant it was declared — a list and not a sentence, so a reference can be
 * read against the wire that carries it.
 *
 * Every figure is the platform's text through `formatDecimal`, which groups
 * digits and never rounds or sums. There is deliberately no total here: the
 * platform's own `expected_inflows_total` is rendered by the caller beside
 * this list, and a list that summed its own rows would be a second figure for
 * the same fact, free to disagree with the first the day the ledger's rule
 * changed.
 */
export function ExpectedInflowList({
  inflows,
  testId,
}: {
  inflows: readonly ExpectedInflow[];
  /** The per-row test id, so the ledger and account pages stay distinguishable. */
  testId: string;
}) {
  if (inflows.length === 0) return null;
  return (
    <ul className="mt-0.5 flex flex-col gap-0.5">
      {inflows.map((inflow) => (
        <li key={inflow.reference} data-testid={testId} data-reference={inflow.reference}>
          <Muted>
            <span className="num">{inflow.reference}</span>: {formatDecimal(inflow.amount)} declared{" "}
            {formatTimestamp(inflow.declared_at)}
          </Muted>
        </li>
      ))}
    </ul>
  );
}

/**
 * The platform's `inflow_posting` sentence, rendered as it came.
 *
 * The contract says render it beside any expected inflow shown, and ADR 0085
 * §2 says why: a declared inflow reads as "arriving" to anyone who sees it,
 * and the blueprint's flow continues `detected → settled → available`, which
 * this build does not. The sentence is the platform's constant and not this
 * console's paraphrase, so that the day the platform starts posting inflows
 * and deletes the constant, the page stops saying it does not — a paraphrase
 * here would go on saying it forever.
 */
export function InflowPostingNote({ sentence, testId }: { sentence: string; testId: string }) {
  return (
    <p className="mt-1 text-[11.5px] leading-relaxed" data-testid={testId}>
      <span className="eyebrow mr-1.5">posting</span>
      <span className="text-[color:var(--color-warn)]">{sentence}</span>
    </p>
  );
}

/**
 * The two routes by which an expected inflow is declared and cancelled, and
 * why this console offers no control for either.
 *
 * The person reading a declared inflow is the person who might want to
 * cancel it, or declare the next one, so the routes are named rather than
 * hidden. But the platform refuses both in fact for every credential it
 * accepts — the routes date the operator by an authentication instant, which
 * a standing bearer token cannot carry (ADR 0065, ADR 0075) — so a form here
 * would be a control whose every submission is refused, and a page that
 * offered one would be pretending to a capability it does not have.
 *
 * What is rendered as the refusal is the route contract's statement and it
 * is labelled as such. `venue_views.rs` serves `reinstatement_refusal` — the
 * platform's own answer for the caller — and the withdrawals page renders
 * that; `ledger_views.rs` serves no such field for these routes, and the one
 * way to obtain the platform's own sentence would be to attempt the write.
 * This console does not. Saying "the platform refused" about a sentence the
 * platform never returned would be the console's prediction wearing the
 * platform's voice; the block says whose sentence it is instead.
 */
export function InflowDeclarationBlock() {
  return (
    <div className="flex flex-col gap-2" data-testid="inflow-declaration">
      <p className="max-w-[90ch] text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
        An expected inflow is declared and cancelled at two operator routes the platform serves
        (ADR 0085):{" "}
        <span className="num" data-testid="inflow-route-declare">
          {INFLOW_ROUTES.declare.method} {INFLOW_ROUTES.declare.path}
        </span>{" "}
        and{" "}
        <span className="num" data-testid="inflow-route-cancel">
          {INFLOW_ROUTES.cancel.method} {INFLOW_ROUTES.cancel.path}
        </span>
        , both at the {INFLOW_ROUTES.role} role. A declaration is a claim that a wire is on its
        way; it receives, posts and invests nothing, and the only thing this build can do with one
        is cancel it. This console offers no form to declare or cancel one and the gateway
        declares no write against either path; the routes are named here because the platform
        serves them, not because this page can call them.
      </p>
      <StateBlock
        tone="warn"
        label="declaration refused"
        headline="A declaration or cancellation at either route is refused for every credential this console could present."
      >
        <p data-testid="inflow-declaration-refusal">{INFLOW_ROUTES.refused_in_fact}</p>
        <p
          className="mt-1.5 text-[color:var(--color-ink-faint)]"
          data-testid="inflow-declaration-refusal-source"
        >
          That is the route contract&rsquo;s statement ({INFLOW_ROUTES.refusal_stated_by}), not an
          answer the platform returned to this page: <span className="num">GET /ledger/users</span>{" "}
          carries no refusal for these routes, and this console does not attempt the write to
          obtain one.
        </p>
      </StateBlock>
    </div>
  );
}
