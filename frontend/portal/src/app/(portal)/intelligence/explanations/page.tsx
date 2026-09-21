"use client";

import Link from "next/link";
import type { ReactNode } from "react";
import { Chip, Freshness, Metric, MetricRow, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, MissingEndpointBlock, ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Models, Orders, SystemStatus } from "@/lib/api/types";
import { QUESTIONS, type Coverage, type Question } from "@/lib/explanations";
import { formatCount, formatDecimal, formatMicros, formatTimestamp } from "@/lib/format";
import { usePrecedents, useSelfModel } from "@/lib/hooks/useCognition";
import { useResource } from "@/lib/hooks/useResource";
import { useRecalibrations } from "@/lib/hooks/useSelfCorrection";

/**
 * Explanations: blueprint §40.2's seven questions, each beside the route that
 * answers it today — or beside the statement that none does.
 *
 * §40.2 draws the line this page is built on: "Attribution says which
 * strategy caused a fill. Explanation says what the system believed and why.
 * Only the second earns trust." The register scored the section two of seven
 * for a year of edits and the two were reachable only by knowing which of
 * forty-odd pages to open; nothing put the seven questions on one screen and
 * said, per question, what the platform can answer. That is what this page
 * does, and it is careful about the word "answer".
 *
 * **Coverage is declared, not computed.** Each question's status —
 * `answered`, `partial`, `absent` — is written into `QUESTIONS` below from
 * what the route contract carries, not inferred from whether a request
 * happened to succeed. A platform that is unreachable does not make a
 * question absent, and a route that answered an empty list does not make it
 * answered; the status says what the platform *can* say, and the live panel
 * under it says what it said just now. The two are kept visibly apart.
 *
 * The seven, with the honesty each one needs:
 *
 * 1. *Why did you take this position?* — half, and it was `absent` until
 *    `GET /proposals` began carrying the hypotheses each leg expresses
 *    beside the rationale sentence. The belief the position rests on is now
 *    named. Its confidence and the evidence that formed it are still on no
 *    wire: a hypothesis id is a handle and no route resolves one.
 * 2. *Why do you believe that?* — half. `GET /cognition/precedents` is the
 *    episodes that resemble now; the causal path through the graph reaches
 *    no route. The register counted the episodes half as the whole question
 *    until this page was written; it is `partial` here.
 * 3. *Why this size?* — half, and the half is named rather than rounded up.
 *    `GET /proposals` now projects the sizing the DECIDE stage recorded: the
 *    weight each leg moved from and to, the reference price it was sized at,
 *    the cost in basis points, and the optimiser's own sentences about what
 *    the result gave up — which name the cap that bound it and the evidence
 *    a bound was narrowed on. That is the sizing as it happened, read off
 *    the record rather than derived. It is still **not** the blueprint's
 *    four terms shown separately: edge, volatility, the grant and the
 *    confidence multiplier are inputs to the optimiser and no route projects
 *    them apart. A page that derived them from a weight would be the second
 *    sizing model in a browser that this note used to say was the reason
 *    nothing could be shown at all.
 * 4. *Why not the obvious trade?* — still half, now for a different reason.
 *    The *gate* half is answered per decision: a vetoed proposal on
 *    `GET /proposals` names the control that vetoed it and the reason it
 *    gave. The route dropped both until this was written, so a console could
 *    report `vetoed` and could not say by what — the one question an
 *    operator asks of a refusal. The counterfactual score of declining is
 *    still aggregated per rule over a window on `GET /risk/recalibrations`
 *    rather than attached to the decision, and `GET /orders` still adds a
 *    count of refusals with no reason per order.
 * 5. *Why this strategy and not that one?* — no route.
 * 6. *What do you not know here?* — answered. `GET /cognition/self-model`
 *    is the platform's stated coverage gaps and the estimates it refuses.
 * 7. *Is this platform sensible at my capital?* — half. Cost is served:
 *    `GET /models` reports what the agents' model use has cost, in the unit
 *    the budget charges. Attributed return is not: `GET /pnl` answers an
 *    absence with the platform's own sentence, which is rendered verbatim
 *    rather than paraphrased. Half a ratio is not a ratio, and this page
 *    does not divide.
 *
 * Nothing here acts. §40.1's Explanation row has "Nothing — understanding"
 * in its "Acts on" cell, every read here is a GET, and the gateway declares
 * no write this page could make.
 *
 * The seven and their declared coverage are `QUESTIONS` in `@/lib/explanations`,
 * a module with no React in it so the behavioural suite can import the table.
 */

const COVERAGE_LABEL: Record<Coverage, string> = {
  answered: "answered",
  partial: "half answered",
  absent: "not answered",
};

const COVERAGE_TONE: Record<Coverage, "ok" | "warn" | "bad"> = {
  answered: "ok",
  partial: "warn",
  absent: "bad",
};

export default function ExplanationsPage() {
  const selfModel = useSelfModel();
  const precedents = usePrecedents();
  const recalibrations = useRecalibrations();
  const orders = useResource<Orders>(platform.orders, {
    key: "explanations-orders",
    label: "GET /orders",
    intervalMs: 15_000,
  });
  const models = useResource<Models>(platform.models, {
    key: "explanations-models",
    label: "GET /models",
    intervalMs: 30_000,
  });
  const pnl = useResource<unknown>(platform.pnl, {
    key: "explanations-pnl",
    label: "GET /pnl",
    intervalMs: 30_000,
  });
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "explanations-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  const answered = QUESTIONS.filter((question) => question.coverage === "answered").length;
  const partial = QUESTIONS.filter((question) => question.coverage === "partial").length;
  const absent = QUESTIONS.filter((question) => question.coverage === "absent").length;

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="explanations-page">
      <Panel>
        <PanelHead
          title="Explanations"
          meta={<Freshness resource={selfModel} name="self-model" />}
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
            data-testid="explanations-declaration"
          >
            <span className="chip mr-2" data-tone="ok" data-testid="explanations-paper-label">
              PAPER TRADING
            </span>
            The seven questions a person asks of a decision, in the blueprint&rsquo;s words, and
            beside each the route that answers it today or the statement that none does. Coverage
            is declared from what each route carries, not from whether it happened to answer just
            now: a platform that is unreachable is unreachable, not unexplained. Nothing on this
            page can act. Explanation is understanding, every read here is a{" "}
            <span className="num">GET</span>, and the gateway declares no write this page could make.
          </p>
          <KpiRow>
            <Kpi
              label="Answered"
              value={<span data-testid="explanations-answered">{formatCount(answered)}</span>}
              note="a route carries the whole of what the question asks"
              tone="ok"
            />
            <Kpi
              label="Half answered"
              value={<span data-testid="explanations-partial">{formatCount(partial)}</span>}
              note="a route carries part of it; the rest is named as missing"
              tone="warn"
            />
            <Kpi
              label="Not answered"
              value={<span data-testid="explanations-absent">{formatCount(absent)}</span>}
              note="no route; the absence is declared with the nearest thing served"
              tone="bad"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      {QUESTIONS.map((question, index) => (
        <QuestionPanel key={question.id} question={question} number={index + 1}>
          {question.id === "belief" ? (
            <ResourceView resource={precedents} loadingRows={2}>
              {(data) => (
                <>
                  <MetricRow>
                    <Metric
                      label="Episodes recalled"
                      value={<span data-testid="explanations-precedents-count">{formatCount(data.precedents.length)}</span>}
                      hint="GET /cognition/precedents: what the memory last recalled"
                    />
                  </MetricRow>
                  <HalfMissing testid="explanations-belief-missing">
                    The causal path through the graph reaches no route. The world model holds
                    causal edges and the LEARN stage scores them; nothing projects the path from a
                    belief to the events that support it, so this half is not shown and not
                    inferred from the episodes.
                  </HalfMissing>
                </>
              )}
            </ResourceView>
          ) : question.id === "declined" ? (
            <>
              <ResourceView resource={recalibrations} loadingRows={3}>
                {(data) =>
                  data.open.length === 0 ? (
                    <EmptyBlock headline="No rule is currently proposed for loosening.">
                      GET /risk/recalibrations answered an empty <span className="num">open</span>{" "}
                      list: no gate has refused enough scored paths to argue for a wider bound.
                    </EmptyBlock>
                  ) : (
                    <ul className="flex flex-col gap-2" data-testid="explanations-regret-list">
                      {data.open.map((proposal) => (
                        <li
                          key={proposal.rule}
                          className="border border-[color:var(--color-line)] px-3 py-2"
                          data-testid="explanations-regret"
                          data-rule={proposal.rule}
                        >
                          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                            <span className="num text-[12px]" data-testid="explanations-regret-rule">
                              {proposal.rule}
                            </span>
                            <Chip>{proposal.kind}</Chip>
                            <span className="text-[11px] text-[color:var(--color-ink-dim)]">
                              refused{" "}
                              <span className="num" data-testid="explanations-regret-sample">
                                {formatCount(proposal.evidence.sample)}
                              </span>{" "}
                              scored paths; the counterfactual said{" "}
                              <span className="num" data-testid="explanations-regret-count">
                                {formatCount(proposal.evidence.regrets)}
                              </span>{" "}
                              were wrong to refuse
                            </span>
                          </div>
                          <div className="mt-1 text-[11px] text-[color:var(--color-ink-dim)]">
                            standing aside cost{" "}
                            <span className="num" data-testid="explanations-regret-earned">
                              {formatDecimal(proposal.evidence.would_have_earned.simulated_value)}
                            </span>{" "}
                            <Chip tone="warn" title="qip_twin::Simulated: a world that did not happen">
                              {proposal.evidence.would_have_earned.simulated ? "simulated" : "NOT FLAGGED SIMULATED"}
                            </Chip>{" "}
                            over {formatTimestamp(proposal.evidence.window[0])} to{" "}
                            {formatTimestamp(proposal.evidence.window[1])}
                          </div>
                        </li>
                      ))}
                    </ul>
                  )
                }
              </ResourceView>
              <ResourceView resource={orders} loadingRows={1}>
                {(data) => (
                  <MetricRow>
                    <Metric
                      label="Orders refused"
                      value={<span data-testid="explanations-refusals">{formatCount(data.refusals)}</span>}
                      hint="GET /orders: a count, with no reason per order"
                    />
                  </MetricRow>
                )}
              </ResourceView>
              <HalfMissing testid="explanations-declined-missing">
                This is the gate that declined and what declining cost, per rule over a window —
                not per declined order. A desk reading it learns which limit has been costing the
                platform, not why one trade was declined; the per-order half reaches no route.
              </HalfMissing>
            </>
          ) : question.id === "unknowns" ? (
            <ResourceView resource={selfModel} loadingRows={2}>
              {(data) => {
                const refused = data.components.filter((component) => !component.calibrated).length;
                return (
                  <MetricRow>
                    <Metric
                      label="Origins measured"
                      value={<span data-testid="explanations-self-model-count">{formatCount(data.components.length)}</span>}
                      hint="detectors, analysts, rungs and families the platform has graded"
                    />
                    <Metric
                      label="Estimates refused"
                      value={<span data-testid="explanations-self-model-refused">{formatCount(refused)}</span>}
                      hint={`below the minimum sample of ${formatCount(data.minimum_sample)}: stated gaps, not zeroes`}
                      tone={refused > 0 ? "warn" : undefined}
                    />
                  </MetricRow>
                );
              }}
            </ResourceView>
          ) : question.id === "cost" ? (
            <>
              <ResourceView resource={models} loadingRows={1}>
                {(data) => (
                  <MetricRow>
                    <Metric
                      label="Model cost"
                      value={<span data-testid="explanations-cost">{formatMicros(data.observed_use.cost_micros)}</span>}
                      hint="GET /models: cost_micros, the unit the budget is charged in"
                    />
                    <Metric
                      label="Model calls"
                      value={formatCount(data.observed_use.model_calls)}
                      hint={`${formatCount(data.observed_use.agent_runs)} agent runs`}
                    />
                  </MetricRow>
                )}
              </ResourceView>
              <div data-testid="explanations-return">
                <ResourceView resource={pnl} loadingRows={1}>
                  {(data) => (
                    // `/pnl` answers an absence today, which `ResourceView`
                    // renders with the platform's own sentence. Should it ever
                    // answer a body, it is shown verbatim under an "unread"
                    // label rather than through a shape this page guessed.
                    <StateBlock tone="info" label="unread" headline="GET /pnl answered a body this page has no columns for.">
                      <pre className="whitespace-pre-wrap break-all text-[11px]">{JSON.stringify(data, null, 2)}</pre>
                    </StateBlock>
                  )}
                </ResourceView>
              </div>
              <HalfMissing testid="explanations-cost-missing">
                Cost is served and attributed return is not, so the ratio the question asks for is
                not on this page: half a ratio is not a ratio, and this console does not divide a
                figure the platform sent by one it did not.
              </HalfMissing>
            </>
          ) : question.missing !== null ? (
            <MissingEndpointBlock endpoint={question.missing} />
          ) : null}
        </QuestionPanel>
      ))}
    </div>
  );
}

function QuestionPanel({
  question,
  number,
  children,
}: {
  question: Question;
  number: number;
  children: ReactNode;
}) {
  return (
    <Panel data-testid="explanations-question" data-question={question.id} data-coverage={question.coverage}>
      <PanelHead
        title={`${number}. ${question.asks}`}
        meta={
          question.reads.length === 0 ? null : (
            <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
              {question.reads.map((path) => `GET ${path}`).join(" · ")}
            </span>
          )
        }
        actions={
          <>
            <StatusChip tone={COVERAGE_TONE[question.coverage]} label={COVERAGE_LABEL[question.coverage]} />
            {question.page === null ? null : (
              <Link href={question.page.href} className="btn" data-variant="ghost">
                {question.page.label}
              </Link>
            )}
          </>
        }
      />
      <PanelBody>
        <p className="mb-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          <span className="eyebrow mr-2">what answers it</span>
          <span data-testid="explanations-answered-by">{question.answeredBy}</span>
        </p>
        <div className="flex flex-col gap-2">{children}</div>
      </PanelBody>
    </Panel>
  );
}

/** The half of a question no route carries, said beside the half one does. */
function HalfMissing({ testid, children }: { testid: string; children: ReactNode }) {
  return (
    <p className="text-[11px] leading-relaxed text-[color:var(--color-warn)]" data-testid={testid}>
      {children}
    </p>
  );
}
