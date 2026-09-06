"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useWallet, type ReconciliationOutcome, type Wallet } from "@/lib/hooks/useTreasury";
import { AbsentBlock, Muted, Quote, TreasuryHeader } from "../_shared";

/**
 * The wallet (blueprint §38), read path only.
 *
 * `GET /wallet` answers what venues reported through read-only channels
 * paired with what the ledger expected of the same venue-asset, and the
 * outcome §38.3's arithmetic produced for each. This page renders the pairing
 * and the outcomes; it computes no delta and applies no correction, because
 * the platform's `Wallet` has no method that could and neither does this
 * console.
 *
 * A `halt` outcome is the loudest thing on the page. It means an external
 * balance the ledger cannot explain — a surplus as much as a shortfall — and
 * the wallet's instruction is to investigate at the venue and the ledger and
 * write nothing. The alert's own message is quoted verbatim, and the halt
 * count is the platform's `halted_venue_assets`, not a count this page took.
 *
 * When no wallet has been assembled the body says `assembled: false` with its
 * reason, and the page says "not assembled" in the platform's words rather
 * than rendering an empty table that would read as a wallet holding nothing.
 */
export default function WalletPage() {
  const wallet = useWallet();

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Wallet"
        reads="GET /wallet"
        posture={wallet.data?.posture ?? null}
        meta={<Freshness resource={wallet} name="wallet" />}
      />

      <Panel>
        <PanelHead title="Reconciliation" />
        <PanelBody>
          <ResourceView resource={wallet} loadingRows={2}>
            {(data) => (data.assembled ? <Reconciliation wallet={data} /> : <NotAssembled wallet={data} />)}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Holdings: observed against the ledger" />
        <PanelBody flush>
          <ResourceView resource={wallet} loadingRows={3}>
            {(data) =>
              !data.assembled && data.holdings.length === 0 ? (
                <div className="p-3">
                  <EmptyBlock headline="No holding, because no wallet is assembled.">
                    <p>The list is empty by consequence of the state above, not because a venue reported nothing.</p>
                  </EmptyBlock>
                </div>
              ) : data.holdings.length === 0 ? (
                <div className="p-3">
                  <EmptyBlock headline="The wallet was assembled from no observation.">
                    <p>No venue reported a balance, so there is nothing to pair with the ledger.</p>
                  </EmptyBlock>
                </div>
              ) : (
                <TableWell maxHeight="480px" label="Holdings observed at venues, beside the ledger's expectation">
                  <table className="dt" data-testid="wallet-holdings">
                    <thead>
                      <tr>
                        <th scope="col">Venue</th>
                        <th scope="col">Asset</th>
                        <th scope="col" className="n">
                          Observed
                        </th>
                        <th scope="col">Observed at</th>
                        <th scope="col">Provenance</th>
                        <th scope="col" className="n">
                          Ledger expected
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.holdings.map((holding) => (
                        <tr key={`${holding.venue}/${holding.asset}`} data-testid="wallet-holding">
                          <td className="num">{holding.venue}</td>
                          <td className="num">{holding.asset}</td>
                          <td className="n">{formatDecimal(holding.observed_quantity)}</td>
                          <td className="num">{formatTimestamp(holding.observed_at)}</td>
                          <td>
                            <Chip>{holding.provenance}</Chip>
                          </td>
                          <td className="n">{formatDecimal(holding.ledger_expected)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
          <p className="px-3 pb-3 pt-2">
            <Muted>
              Ledger expected is the platform&rsquo;s ledger balance less reserved plus in flight
              (§38.3). Provenance names the class of read-only channel, never the channel: the read
              path holds nothing that could move a unit, and neither does this page.
            </Muted>
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}

function NotAssembled({ wallet }: { wallet: Wallet }) {
  return (
    <>
      <AbsentBlock
        label="not assembled"
        headline="No wallet has been assembled in this process."
        reason={wallet.reason}
        testId="wallet-not-assembled"
      />
      <p className="mt-2">
        <Muted>
          A wallet is assembled from venue observations and ledger views the platform is given;
          nothing here can supply either. Until it is, there is no holding to show and no
          reconciliation to report. Served {formatTimestamp(wallet.served_at)}.
        </Muted>
      </p>
      {wallet.reconciliation.outcomes.length > 0 ? (
        // The body says no wallet is assembled and also carries outcomes. Both
        // are shown rather than one chosen; two claims that disagree are the
        // operator's finding, not this page's to resolve.
        <div className="mt-3">
          <Quote tone="warn">
            The body reports no wallet assembled and also carries{" "}
            {formatCount(wallet.reconciliation.outcomes.length)} reconciliation outcome(s). Both are
            shown below.
          </Quote>
          <OutcomeTable outcomes={wallet.reconciliation.outcomes} />
        </div>
      ) : null}
    </>
  );
}

function Reconciliation({ wallet }: { wallet: Wallet }) {
  const outcomes = wallet.reconciliation.outcomes;
  const halts = outcomes.filter((outcome) => outcome.outcome === "halt");
  return (
    <>
      <KpiRow>
        <Kpi
          label="Venue-assets assessed"
          value={formatCount(outcomes.length)}
          note={`${formatCount(outcomes.length - halts.length)} reconciled inside tolerance`}
          tone={outcomes.length === 0 ? "neutral" : "ok"}
        />
        <Kpi
          label="HALTED"
          value={<span data-testid="wallet-halt-count">{formatCount(wallet.reconciliation.halted_venue_assets)}</span>}
          note={
            wallet.reconciliation.halted_venue_assets === 0
              ? "no venue-asset is halted; the platform's own count"
              : "investigate; the wallet writes no correction"
          }
          tone={wallet.reconciliation.halted_venue_assets > 0 ? "bad" : "ok"}
        />
        <Kpi label="As of" value={formatTimestamp(wallet.as_of)} note="the instant the wallet was assembled against" />
      </KpiRow>
      {halts.length > 0 ? (
        <div className="mt-3 flex flex-col gap-2" role="alert" data-testid="wallet-halts">
          {halts.map((outcome) =>
            outcome.outcome === "halt" ? (
              <div
                key={`${outcome.venue}/${outcome.asset}`}
                className="border border-[color:var(--color-down)] px-3 py-2"
                data-alert="true"
              >
                <div className="flex flex-wrap items-center gap-2">
                  <Chip tone="bad">HALT</Chip>
                  <span className="num text-[13px] font-semibold">
                    {outcome.venue}/{outcome.asset}
                  </span>
                  <Chip tone="warn">{outcome.alert.cause}</Chip>
                  <span className="num text-[11px] text-[color:var(--color-ink-faint)]">
                    expected {formatDecimal(outcome.alert.expected)} · observed {formatDecimal(outcome.alert.observed)} ·
                    delta {formatDecimal(outcome.alert.delta)} · tolerance {formatDecimal(outcome.alert.tolerance)} · via{" "}
                    {outcome.alert.provenance}
                  </span>
                </div>
                <Quote tone="bad">{outcome.alert.message}</Quote>
              </div>
            ) : null,
          )}
        </div>
      ) : null}
      {outcomes.length === 0 ? (
        <div className="mt-3">
          <EmptyBlock headline="The wallet holds no venue-asset to reconcile." />
        </div>
      ) : (
        <div className="mt-3">
          <OutcomeTable outcomes={outcomes} />
        </div>
      )}
    </>
  );
}

function OutcomeTable({ outcomes }: { outcomes: readonly ReconciliationOutcome[] }) {
  return (
    <TableWell maxHeight="320px" label="Reconciliation outcome per venue-asset">
      <table className="dt" data-testid="wallet-outcomes">
        <thead>
          <tr>
            <th scope="col">Venue</th>
            <th scope="col">Asset</th>
            <th scope="col">Outcome</th>
            <th scope="col" className="n">
              Delta (observed − expected)
            </th>
            <th scope="col">Detail</th>
          </tr>
        </thead>
        <tbody>
          {outcomes.map((outcome) => (
            <tr
              key={`${outcome.venue}/${outcome.asset}`}
              data-testid="wallet-outcome"
              data-alert={outcome.outcome === "halt" ? "true" : undefined}
            >
              <td className="num">{outcome.venue}</td>
              <td className="num">{outcome.asset}</td>
              <td>
                <Chip tone={outcome.outcome === "halt" ? "bad" : "ok"}>
                  {outcome.outcome === "halt" ? "HALT" : "reconciled"}
                </Chip>
              </td>
              <td className="n">{formatDecimal(outcome.delta)}</td>
              <td className="max-w-[48ch] text-[11px] text-[color:var(--color-ink-dim)]">
                {outcome.outcome === "halt" ? outcome.alert.message : "inside tolerance"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </TableWell>
  );
}
