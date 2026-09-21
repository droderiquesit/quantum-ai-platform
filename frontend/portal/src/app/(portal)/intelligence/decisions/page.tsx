"use client";

import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Proposal, ProposalDecision, Proposals, SystemStatus } from "@/lib/api/types";
import { formatCount, formatDecimal, formatPercent, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The decision record: what the DECIDE stage proposed, how it sized it, what
 * it gave up, and which control decided it.
 *
 * Every figure here is read off `GET /proposals`. **Nothing on this page is
 * computed**, and that constraint is the reason the page exists at all rather
 * than being a rearrangement of a list the console already had: blueprint
 * §40.2 asks "why this size?", the four terms behind an answer live in the
 * DECIDE stage, and a page that derived them from a target weight would be a
 * second sizing model running in a browser — the exact failure
 * `.claude/rules/domains/frontend.md` forbids. So the route was widened to
 * project what the platform recorded, and this page renders it.
 *
 * **What was actually wrong before.** `GET /proposals` served six fields: an
 * id, a status *word*, a leg count, gross, turnover and a rationale sentence.
 * The `Proposal` behind it already carried the control that vetoed it and the
 * reason, the weights each leg was sized between, the reference price it was
 * sized at, the hypotheses it expresses and what the construction had to
 * compromise on. A console reading the old body could say that a proposal was
 * `vetoed` and could not say by what or why. The platform had written the
 * answer down and the wire discarded it.
 *
 * **The compromises are the honest part and are rendered verbatim.** They are
 * sentences the optimiser wrote about its own result — that a cap won, that a
 * bound was narrowed on counterfactual evidence, that the book is
 * deliberately under-invested. They are not summarised, ranked or counted
 * into a score here: each is a judgement the platform already made, and a
 * second judgement formed in a browser can disagree with the one that acted.
 *
 * There is no control on this page. Every read is a `GET`, the gateway
 * declares no write it could call, and nothing here submits, approves or
 * cancels anything.
 */

/** The controls a decision names, as one line. Never invented when absent. */
function decidedBy(decision: ProposalDecision | undefined): string | null {
  if (!decision || decision.by === undefined) return null;
  return Array.isArray(decision.by) ? decision.by.join(", ") : String(decision.by);
}

function toneFor(status: string): "ok" | "warn" | "bad" | "neutral" {
  if (status === "vetoed") return "bad";
  if (status === "withdrawn") return "warn";
  if (status === "approved" || status === "released") return "ok";
  return "neutral";
}

function DecisionCard({ proposal }: { proposal: Proposal }) {
  const legs = proposal.leg_detail ?? [];
  const compromises = proposal.compromises ?? [];
  const checks = proposal.checks_passed ?? [];
  const by = decidedBy(proposal.decision);

  return (
    <div
      className="rounded-xl border border-border bg-panel p-4 flex flex-col gap-3"
      data-testid="decision-card"
      data-proposal={proposal.id}
    >
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <span className="num text-[12px] text-[color:var(--color-ink)]">{proposal.id}</span>
        {/* Wrapped rather than given a test id directly: `Chip` does not
            spread unknown props, and a hyphenated JSX attribute skips
            TypeScript's excess-property check — so the attribute would have
            been dropped silently and a test written against it would have
            been asserting on nothing. */}
        <span data-testid="decision-status">
          <Chip tone={toneFor(proposal.status)}>{proposal.status}</Chip>
        </span>
      </div>

      <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="decision-rationale">
        {proposal.rationale}
      </p>

      {/* Who decided it and when. A draft has not been reviewed, and saying
          so is different from naming nobody. */}
      <p className="text-[11.5px] text-[color:var(--color-ink-faint)]" data-testid="decision-by">
        {proposal.decision === undefined
          ? "This platform served no decision record for this proposal."
          : proposal.decision.status === "draft"
            ? "Drafted, not yet reviewed — no control has ruled on it."
            : `${proposal.decision.status} at ${formatTimestamp(proposal.decision.at)}${by === null ? "" : ` by ${by}`}`}
      </p>

      {/* The refusal, in the platform's own words. This is the field whose
          absence made "why not the obvious trade?" unanswerable. */}
      {proposal.decision?.reason === undefined ? null : (
        <p
          className="text-[12px] leading-relaxed text-[color:var(--color-ink)] border-l-2 border-red-500/60 pl-3"
          data-testid="decision-reason"
        >
          {proposal.decision.reason}
        </p>
      )}

      <KpiRow>
        <Kpi label="Gross" value={formatPercent(proposal.gross)} note="target gross exposure" />
        <Kpi label="Net" value={formatPercent(proposal.target_net)} note="target net exposure" />
        <Kpi label="Turnover" value={formatPercent(proposal.turnover)} note="fraction of the book that must trade" />
        <Kpi
          label="Equity"
          value={<span data-testid="decision-equity">{formatDecimal(proposal.equity)}</span>}
          note="the book the weights are fractions of"
        />
      </KpiRow>

      {/* ── Sizing, per leg ─────────────────────────────────────────────── */}
      {legs.length === 0 ? (
        <p className="text-[11.5px] text-[color:var(--color-ink-faint)]" data-testid="decision-no-legs">
          This proposal sized nothing. The cycle ran and no thesis cleared the action bar, which the
          rationale above states in the platform&rsquo;s own words.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-[11.5px]" data-testid="decision-legs">
            <thead className="text-[color:var(--color-ink-faint)] text-left">
              <tr>
                <th className="py-1 pr-3 font-medium">Instrument</th>
                <th className="py-1 pr-3 font-medium">Side</th>
                <th className="py-1 pr-3 font-medium">Weight from</th>
                <th className="py-1 pr-3 font-medium">to</th>
                <th className="py-1 pr-3 font-medium">Move</th>
                <th className="py-1 pr-3 font-medium">Quantity</th>
                <th className="py-1 pr-3 font-medium">Reference price</th>
                <th className="py-1 pr-3 font-medium">Cost (bps)</th>
                <th className="py-1 font-medium">Hypotheses</th>
              </tr>
            </thead>
            <tbody className="text-[color:var(--color-ink-dim)]">
              {legs.map((leg) => (
                <tr key={`${proposal.id}-${leg.instrument}`} className="border-t border-border">
                  <td className="py-1.5 pr-3 num text-[color:var(--color-ink)]">{leg.instrument}</td>
                  <td className="py-1.5 pr-3">{leg.side}</td>
                  <td className="py-1.5 pr-3 num">{formatPercent(leg.current_weight)}</td>
                  <td className="py-1.5 pr-3 num">{formatPercent(leg.target_weight)}</td>
                  <td className="py-1.5 pr-3 num">{formatPercent(leg.weight_change)}</td>
                  {/* Exact decimals, rendered as they arrived. */}
                  <td className="py-1.5 pr-3 num">{formatDecimal(leg.quantity)}</td>
                  <td className="py-1.5 pr-3 num">{formatDecimal(leg.reference_price)}</td>
                  <td className="py-1.5 pr-3 num">{leg.estimated_cost_bps}</td>
                  <td className="py-1.5 num">{leg.hypotheses.join(", ") || "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* ── What it gave up ─────────────────────────────────────────────── */}
      {compromises.length === 0 ? null : (
        <div data-testid="decision-compromises">
          <span className="eyebrow">what the construction gave up</span>
          <ul className="mt-1 flex flex-col gap-1">
            {compromises.map((compromise) => (
              <li
                key={compromise}
                className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)] border-l-2 border-amber-500/50 pl-3"
              >
                {compromise}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex items-center gap-2 flex-wrap">
        <span className="eyebrow">controls passed</span>
        {checks.length === 0 ? (
          <span className="text-[11.5px] text-[color:var(--color-ink-faint)]" data-testid="decision-no-checks">
            none recorded
          </span>
        ) : (
          checks.map((check) => (
            <Chip key={check} tone="ok">
              {check}
            </Chip>
          ))
        )}
        {proposal.as_of === undefined ? null : (
          <span className="text-[11px] text-[color:var(--color-ink-faint)] ml-auto">
            sized as of {formatTimestamp(proposal.as_of)}
          </span>
        )}
      </div>
    </div>
  );
}

export default function DecisionsPage() {
  const proposals = useResource<Proposals>(platform.proposals, {
    key: "decisions-proposals",
    label: "GET /proposals",
    intervalMs: 10_000,
  });
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "decisions-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="decisions-page">
      <Panel>
        <PanelHead
          title="Decision record"
          meta={<Freshness resource={proposals} name="proposals" />}
          actions={
            status.data === null ? null : (
              <StatusChip
                tone={status.data.live_capable ? "bad" : "ok"}
                label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /system/status: live_capable"
              />
            )
          }
        />
        <PanelBody>
          <p
            className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
            data-testid="decisions-declaration"
          >
            <span className="chip mr-2" data-tone="ok" data-testid="decisions-paper-label">
              PAPER TRADING
            </span>
            This page reads <span className="num">GET /proposals</span> and renders what it answered.
            Every weight, price and compromise below was recorded by the DECIDE stage; none of it is
            derived here. Nothing on this page submits, approves, cancels or sizes anything — the
            gateway declares no write it could call.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Proposals" meta={<Freshness resource={proposals} name="decision record" />} />
        <PanelBody>
          <ResourceView resource={proposals} loadingRows={3}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Proposals"
                    value={<span data-testid="decisions-count">{formatCount(data.proposals.length)}</span>}
                    note="staged by the loop, newest last"
                  />
                  <Kpi
                    label="Refused"
                    value={formatCount(data.proposals.filter((p) => p.status === "vetoed").length)}
                    note="vetoed by a control; each names which and why"
                  />
                  <Kpi
                    label="Released"
                    value={formatCount(data.proposals.filter((p) => p.status === "released").length)}
                    note="handed to execution against the simulator"
                  />
                </KpiRow>

                {data.proposals.length === 0 ? (
                  <div className="mt-3" data-testid="decisions-empty">
                    <EmptyBlock headline="The loop has staged no proposal.">
                      <p>
                        Observed, not unread: the route answered and its{" "}
                        <code className="num">proposals</code> list is empty. A proposal appears when
                        a cycle reaches its DECIDE stage; none has run in this process yet.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3 flex flex-col gap-3">
                    {[...data.proposals].reverse().map((proposal) => (
                      <DecisionCard key={proposal.id} proposal={proposal} />
                    ))}
                  </div>
                )}
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
