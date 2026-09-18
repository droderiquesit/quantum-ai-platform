"use client";

import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { formatCount, formatPercent, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { useVenueWithdrawals, type WithdrawnVenueRow } from "@/lib/hooks/useSelfCorrection";

/**
 * Venue withdrawals: which venues the platform stopped using on its own
 * feasibility evidence, the cluster each was withdrawn on, and where the way
 * back stands.
 *
 * `GET /venues/withdrawals` (ADR 0062) answers the withdrawn set, how many
 * withdrawal records the log holds in all, the path a reinstatement is signed
 * at, and — for the credential this deployment holds — why a signature at
 * that path would be refused. All four are rendered. The fourth is the one
 * that matters most on a screen: the route publishes the path because the
 * person reading the list is the person about to call it, and a console that
 * showed the path without the refusal would hand an operator a call that
 * cannot succeed in the middle of a recovery.
 *
 * **Why an empty list is the interesting case.** A venue withdrawal is a
 * fail-closed control: the platform can stop trading its only venue on its
 * own evidence, and ADR 0062 accepts that consequence deliberately. So the
 * two states a desk must be able to tell apart are "nothing has ever been
 * withdrawn" and "something was withdrawn and two people put it back", and
 * they look identical in the `withdrawn` list alone. `withdrawals_recorded`
 * separates them, and this page shows it beside the list for that reason.
 *
 * Nothing on this page acts. Reinstatement is `POST
 * /venues/{venue}/reinstatements`, it needs an operator whose presence was
 * attested, and the gateway refuses any write this console does not declare.
 * The share, the sample and the count are the review's arithmetic, rendered;
 * this page divides nothing.
 */
export default function VenueWithdrawalsPage() {
  const withdrawals = useVenueWithdrawals();
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "venue-withdrawals-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="venue-withdrawals-page">
      <Panel>
        <PanelHead
          title="Venue withdrawals"
          meta={<Freshness resource={withdrawals} name="withdrawals" />}
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
            data-testid="venue-withdrawals-declaration"
          >
            <span className="chip mr-2" data-tone="ok" data-testid="venue-withdrawals-paper-label">
              PAPER TRADING
            </span>
            This page reads <span className="num">GET /venues/withdrawals</span> and renders what it
            answered. Nothing here withdraws a venue, reinstates one, permits one or submits an
            order: the platform withdraws a venue itself, in its LEARN stage, on feasibility
            refusals it journaled, and putting one back is two operators&rsquo; signatures at a route
            the gateway declares no write against. Reinstatement removes a name from a subtractive
            set and can never add one.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Withdrawn now" meta={<Freshness resource={withdrawals} name="withdrawn set" />} />
        <PanelBody>
          <ResourceView resource={withdrawals} loadingRows={4}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Withdrawn now"
                    value={<span data-testid="venue-withdrawals-count">{formatCount(data.withdrawn.length)}</span>}
                    note="venues the desk and the cells will not route to, read from the set this process resumed"
                    tone={data.withdrawn.length > 0 ? "bad" : "ok"}
                  />
                  <Kpi
                    label="Records in the log"
                    value={
                      <span data-testid="venue-withdrawals-recorded">
                        {formatCount(data.withdrawals_recorded)}
                      </span>
                    }
                    note="withdrawals ever journaled, including venues since put back — this is what tells never-withdrawn from withdrawn-and-restored"
                  />
                  <Kpi
                    label="Awaiting a countersignature"
                    value={
                      <span data-testid="venue-withdrawals-awaiting">
                        {formatCount(data.withdrawn.filter((row) => row.awaiting_countersignature).length)}
                      </span>
                    }
                    note="rows where a first signature is standing; who signed is on the event log, not on this list"
                    tone={
                      data.withdrawn.some((row) => row.awaiting_countersignature) ? "warn" : "neutral"
                    }
                  />
                </KpiRow>

                {data.withdrawn.length === 0 ? (
                  <div className="mt-3" data-testid="venue-withdrawals-empty">
                    <EmptyBlock headline="No venue is withdrawn.">
                      <p>
                        Observed, not unread: the route answered and the withdrawn set is empty, so
                        every venue this deployment is configured and granted for is still routable.
                        The log holds{" "}
                        <span className="num">{formatCount(data.withdrawals_recorded)}</span>{" "}
                        withdrawal record(s) in all
                        {data.withdrawals_recorded === 0
                          ? ", so nothing has ever been withdrawn here."
                          : ", so a venue has been withdrawn before and is no longer."}
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3 flex flex-col gap-2" data-testid="venue-withdrawals-list">
                    {data.withdrawn.map((row) => (
                      <WithdrawnVenueCard key={row.venue} row={row} />
                    ))}
                  </div>
                )}
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="The way back" />
        <PanelBody>
          <ResourceView resource={withdrawals} loadingRows={2}>
            {(data) => (
              <div className="flex flex-col gap-2">
                <p className="max-w-[90ch] text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  A withdrawn venue is put back by two different people signing{" "}
                  <span className="num" data-testid="venue-withdrawals-reinstatement-path">
                    POST {data.reinstatement_path}
                  </span>
                  , each with a rationale and nothing else &mdash; the approver is the authenticated
                  session, because an approval a caller can name is not an approval. This console
                  offers no control for it and the gateway declares no such write; the path is here
                  because the platform serves it, not because this page can call it.
                </p>
                {data.reinstatement_refusal === null ? (
                  <StateBlock
                    tone="info"
                    label="signature"
                    headline="This deployment's credential would be admitted at the signature route."
                  >
                    <p data-testid="venue-withdrawals-refusal-absent">
                      The platform reported no refusal for this caller. The signature still has to be
                      made by an operator, from somewhere that is not this console, and still needs a
                      second person.
                    </p>
                  </StateBlock>
                ) : (
                  <StateBlock
                    tone="warn"
                    label="signature refused"
                    headline="A signature at that path would be refused for this deployment's credential."
                  >
                    <p className="num" data-testid="venue-withdrawals-refusal">
                      {data.reinstatement_refusal}
                    </p>
                    <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
                      The platform&rsquo;s own answer, obtained by making the same call the signature
                      route makes. It is not this console&rsquo;s prediction about the route.
                    </p>
                  </StateBlock>
                )}
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * One withdrawn venue and the cluster it was withdrawn on.
 *
 * A `withdrawal` of `null` is rendered as the retention fact it is. It means
 * the log no longer holds the record the withdrawn set was resumed from — the
 * venue is still withdrawn, and the evidence is simply no longer readable
 * from here. Showing a blank row, or worse a zero share, would read as "no
 * evidence", which is the opposite fact.
 */
function WithdrawnVenueCard({ row }: { row: WithdrawnVenueRow }) {
  return (
    <article
      className="border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-3"
      data-testid="venue-withdrawals-row"
      data-venue={row.venue}
      data-awaiting={row.awaiting_countersignature ? "true" : "false"}
    >
      <header className="flex flex-wrap items-center gap-2">
        <span className="num text-[13px] font-semibold text-[color:var(--color-ink)]">{row.venue}</span>
        <Chip tone="bad">withdrawn</Chip>
        {row.awaiting_countersignature ? (
          <Chip tone="warn" title="a first signature is standing; a second, from a different person, puts the venue back">
            <span data-testid="venue-withdrawals-awaiting-chip">awaiting countersignature</span>
          </Chip>
        ) : null}
      </header>

      {row.withdrawal === null ? (
        <div className="mt-2" data-testid="venue-withdrawals-record-absent">
          <StateBlock
            tone="warn"
            label="record not retained"
            headline="The log no longer holds the record this withdrawal was resumed from."
            compact
          >
            <p>
              The venue is withdrawn &mdash; that is the set this process resumed &mdash; and the
              cluster it was withdrawn on has aged out of retention. Nothing is shown in its place:
              a blank share would read as no evidence, which is a different fact from evidence that
              is no longer readable.
            </p>
          </StateBlock>
        </div>
      ) : (
        <>
          <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-[11.5px] sm:grid-cols-5">
            <Fact
              label="Dominating constraint"
              value={<span data-testid="venue-withdrawals-constraint">{row.withdrawal.constraint}</span>}
            />
            <Fact
              label="Refusals"
              value={<span data-testid="venue-withdrawals-refusal-count">{formatCount(row.withdrawal.count)}</span>}
            />
            <Fact
              label="Judged against"
              value={<span data-testid="venue-withdrawals-sample">{formatCount(row.withdrawal.sample)}</span>}
            />
            <Fact
              label="Share"
              value={<span data-testid="venue-withdrawals-share">{formatPercent(row.withdrawal.share)}</span>}
              note="the review's own figure"
            />
            <Fact label="Cycle" value={formatCount(row.withdrawal.cycle)} />
          </dl>
          <p className="mt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            Withdrawn at <span className="num">{formatTimestamp(row.withdrawal.at)}</span> on refusals
            from{" "}
            <span data-testid="venue-withdrawals-seams">
              {row.withdrawal.seams.length === 0 ? "no seam the record names" : row.withdrawal.seams.join(", ")}
            </span>
            . A cluster the desk alone saw may be the desk&rsquo;s own grid being wrong, which is a
            different fault from a venue that refuses everyone &mdash; so the seams are shown rather
            than summed.
          </p>
        </>
      )}
    </article>
  );
}

function Fact({ label, value, note }: { label: string; value: React.ReactNode; note?: string }) {
  return (
    <div className="min-w-0">
      <dt className="eyebrow truncate">{label}</dt>
      <dd className="num text-[13px] text-[color:var(--color-ink)]">{value}</dd>
      {note ? <p className="text-[10.5px] leading-snug text-[color:var(--color-ink-faint)]">{note}</p> : null}
    </div>
  );
}
