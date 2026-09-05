"use client";

import { Chip, Freshness, KeyValue, Metric, MetricRow } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { BacktestRecord, Backtests, LedgerMove } from "@/lib/api/types";
import { isUnavailable } from "@/lib/api/types";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The backtester's record, as the strategy ledger holds it.
 *
 * `GET /api/v1/backtests` serves, per strategy, the holdout evidence that was
 * submitted, the trial account it was charged under, the band the holdout
 * gate produced, and every move on the ledger with the gate findings that
 * admitted it. Nothing here is recomputed by the platform and nothing is
 * computed by this page.
 *
 * Two things the route says it does not serve, and this page therefore does
 * not draw. There is no numeric deflated Sharpe: the gate wrote it into its
 * own `deflated_sharpe_above_selection` finding, which is rendered under the
 * ledger move that carries it. And there is **no equity curve**, because the
 * ledger keeps none; the seeded curve this page used to show under a
 * SIMULATED DATA banner is gone, and its panel now carries the platform's
 * reason instead. A curve drawn here would be one nobody computed.
 */

function num(value: number, digits = 3): string {
  return Number.isFinite(value) ? value.toFixed(digits) : "—";
}

function Holdout({ record }: { record: BacktestRecord }) {
  const holdout = record.holdout;
  if (!holdout.submitted) {
    return (
      <p className="text-[11px] text-[color:var(--color-ink-faint)]" data-testid="holdout-not-submitted">
        No holdout evidence has been submitted for this strategy.
      </p>
    );
  }
  return (
    <MetricRow>
      <Metric label="Observations" value={formatCount(holdout.observations)} hint="holdout returns" />
      <Metric label="Trials this run" value={formatCount(holdout.trials_this_run)} />
      <Metric label="Periods / year" value={num(holdout.periods_per_year, 0)} />
      <Metric
        label="Cross-validation"
        value={`${formatCount(holdout.cross_validation.folds)} folds`}
        hint={`${formatCount(holdout.cross_validation.observations)} obs · ${formatCount(holdout.cross_validation.purged)} purged · ${formatCount(holdout.cross_validation.embargoed)} embargoed`}
      />
      <Metric
        label="Leakage findings"
        value={formatCount(holdout.leakage_findings.length)}
        tone={holdout.leakage_findings.length > 0 ? "warn" : "ok"}
        hint={holdout.leakage_findings.length > 0 ? holdout.leakage_findings.join("; ") : "none recorded"}
      />
    </MetricRow>
  );
}

function Band({ record }: { record: BacktestRecord }) {
  const band = record.holdout_band;
  if (!band.present) {
    return (
      <p className="text-[11px] text-[color:var(--color-ink-dim)]" data-testid="band-absent">
        {band.reason}
      </p>
    );
  }
  return (
    <MetricRow>
      <Metric label="Holdout Sharpe" value={num(band.sharpe)} hint="annualised; the band's centre" />
      <Metric label="Band" value={`${num(band.lower)} … ${num(band.upper)}`} hint={`standard error ${num(band.standard_error)}`} />
      <Metric label="Observations" value={formatCount(band.observations)} />
      <Metric label="Trials" value={formatCount(band.trials)} hint="the count the gate deflated against" />
      <Metric label="Method" value={band.method} hint={`as of ${formatTimestamp(band.as_of)}`} />
    </MetricRow>
  );
}

function Ledger({ moves }: { moves: readonly LedgerMove[] }) {
  if (moves.length === 0) {
    return (
      <p className="text-[11px] text-[color:var(--color-ink-faint)]">The ledger records no move for this strategy.</p>
    );
  }
  return (
    <TableWell maxHeight="360px" label="Ledger moves and the gate findings that admitted them">
      <table className="dt">
        <thead>
          <tr>
            <th scope="col">Move</th>
            <th scope="col">At</th>
            <th scope="col">Approver</th>
            <th scope="col">Rationale</th>
            <th scope="col">Gate</th>
            <th scope="col">Findings</th>
          </tr>
        </thead>
        <tbody>
          {moves.map((move, index) => (
            <tr key={`${move.at}:${index}`} data-testid="ledger-move">
              <td className="num">
                {move.from} → {move.to}
              </td>
              <td className="num">{formatTimestamp(move.at)}</td>
              <td className="num">{move.approver ?? "—"}</td>
              <td className="max-w-[36ch] text-[11px] text-[color:var(--color-ink-dim)]">{move.rationale}</td>
              <td>
                {move.gate === null ? (
                  <span className="text-[11px] text-[color:var(--color-ink-faint)]">no gate outcome</span>
                ) : (
                  <Chip tone={move.gate.passed ? "ok" : "bad"}>
                    {move.gate.stage} · {move.gate.passed ? "passed" : "failed"}
                  </Chip>
                )}
              </td>
              <td>
                {move.gate === null || move.gate.findings.length === 0 ? (
                  <span className="text-[11px] text-[color:var(--color-ink-faint)]">—</span>
                ) : (
                  <ul className="flex flex-col gap-1">
                    {move.gate.findings.map((finding) => (
                      <li key={finding.check} className="text-[11px]" data-testid="gate-finding">
                        <Chip tone={finding.passed ? "ok" : "bad"}>{finding.check}</Chip>{" "}
                        <span className="text-[color:var(--color-ink-dim)]">{finding.detail}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </TableWell>
  );
}

function StrategyPanel({ record }: { record: BacktestRecord }) {
  const account = record.trial_account;
  return (
    <Panel data-testid="backtest-strategy">
      <PanelHead
        title={record.strategy}
        meta={
          <>
            <Chip tone="info">{record.family}</Chip>
            <Chip>{record.cell}</Chip>
            <Chip>{record.venue}</Chip>
            <Chip tone="ok">PAPER TRADING</Chip>
          </>
        }
        actions={<Chip tone="neutral">stage · {record.stage}</Chip>}
      />
      <PanelBody>
        <div className="flex flex-col gap-3">
          <section>
            <span className="eyebrow">Holdout evidence submitted</span>
            <Holdout record={record} />
          </section>
          <section>
            <span className="eyebrow">Trial account</span>
            {account.on_evidence ? (
              <MetricRow>
                <Metric label="Lifetime" value={formatCount(account.lifetime)} />
                <Metric label="This run" value={formatCount(account.this_run)} />
                <Metric label="Prior" value={formatCount(account.prior)} />
                <Metric label="Charged at" value={formatTimestamp(account.charged_at)} />
              </MetricRow>
            ) : (
              <p className="text-[11px] text-[color:var(--color-ink-dim)]" data-testid="trial-account-absent">
                {account.reason}
              </p>
            )}
            <p className="mt-1 text-[11px] text-[color:var(--color-ink-faint)]">
              Family lifetime trials on the book:{" "}
              <span className="num">{formatCount(record.family_lifetime_trials)}</span>
              {record.family_lifetime_trials === null ? " (the book holds no count for this family)" : ""}
            </p>
          </section>
          <section>
            <span className="eyebrow">Holdout band</span>
            <Band record={record} />
          </section>
          <section>
            <span className="eyebrow">Ledger</span>
            <Ledger moves={record.ledger} />
          </section>
          <p className="text-[10.5px] text-[color:var(--color-ink-faint)]">
            Registered {formatTimestamp(record.registered_at)}.
          </p>
        </div>
      </PanelBody>
    </Panel>
  );
}

export default function BacktestingPage() {
  const backtests = useResource<Backtests>(platform.backtests, {
    key: "backtests",
    label: "GET /backtests",
    intervalMs: 20_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Strategies on the ledger"
          meta={<Freshness resource={backtests} name="backtests" />}
          actions={<Chip>GET /api/v1/backtests</Chip>}
        />
        <PanelBody>
          <ResourceView resource={backtests} loadingRows={5}>
            {(data) =>
              data.strategies.length === 0 ? (
                <EmptyBlock headline="Nothing is registered with the strategy factory in this process.">
                  <p data-testid="backtests-empty">
                    <code className="num">GET /api/v1/backtests</code> answered with an empty list — a
                    measured zero, not a failed read. The factory is held in this process, so an
                    empty list means no candidate has been registered, no holdout evidence
                    submitted, and no gate has run. Nothing is shown in its place.
                  </p>
                </EmptyBlock>
              ) : (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi label="Strategies" value={formatCount(data.strategies.length)} note="registered with the factory" />
                    <Kpi
                      label="With holdout evidence"
                      value={formatCount(data.strategies.filter((s) => s.holdout.submitted).length)}
                      note="evidence submitted to the gate"
                    />
                    <Kpi
                      label="With a band"
                      value={formatCount(data.strategies.filter((s) => s.holdout_band.present).length)}
                      tone="ok"
                      note="admitted through the holdout gate"
                    />
                  </KpiRow>
                  {data.strategies.map((record) => (
                    <StrategyPanel key={record.strategy} record={record} />
                  ))}
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Trial book" />
          <PanelBody>
            <ResourceView resource={backtests} loadingRows={3}>
              {(data) =>
                data.trial_book.attached ? (
                  <div className="flex flex-col gap-2" data-testid="trial-book">
                    <MetricRow>
                      <Metric label="Attached" value="yes" tone="ok" />
                      <Metric
                        label="Durable"
                        value={data.trial_book.durable ? "yes" : "no"}
                        tone={data.trial_book.durable ? "ok" : "warn"}
                        hint={data.trial_book.durable ? "survives a restart" : "in-process only"}
                      />
                      <Metric label="Families" value={formatCount(data.trial_book.families.length)} />
                    </MetricRow>
                    {data.trial_book.families.length === 0 ? (
                      <p className="text-[11px] text-[color:var(--color-ink-faint)]">
                        The book names no family: no holdout evaluation has been charged in this
                        process&apos;s lifetime.
                      </p>
                    ) : (
                      <dl>
                        {data.trial_book.families.map((family) => (
                          <KeyValue key={family.family} label={family.family}>
                            {formatCount(family.lifetime_trials)} lifetime trial(s)
                          </KeyValue>
                        ))}
                      </dl>
                    )}
                  </div>
                ) : (
                  <p className="text-[11px] text-[color:var(--color-ink-dim)]" data-testid="trial-book-detached">
                    No trial book is attached to the ledger in this process.
                  </p>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="What the ledger does not keep" actions={<Chip tone="warn">not served</Chip>} />
          <PanelBody>
            <ResourceView resource={backtests} loadingRows={3}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <div data-testid="equity-curve-absent">
                    {isUnavailable(data.equity_curve) ? (
                      <UnavailableBlock subject="equity curve" reason={data.equity_curve.reason} />
                    ) : (
                      <p className="text-[11px] text-[color:var(--color-ink-dim)]">
                        The route now reports an equity curve as available. This console has no
                        renderer for it yet and draws nothing in its place.
                      </p>
                    )}
                  </div>
                  <div data-testid="deflated-sharpe-absent">
                    {isUnavailable(data.deflated_sharpe) ? (
                      <UnavailableBlock subject="numeric deflated Sharpe" reason={data.deflated_sharpe.reason} />
                    ) : (
                      <p className="text-[11px] text-[color:var(--color-ink-dim)]">
                        The route now reports a deflated Sharpe as available. This console has no
                        renderer for it yet.
                      </p>
                    )}
                  </div>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
