"use client";

import { Chip, Freshness, KeyValue, type Tone } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { formatSeconds, useCorridors, type Corridor } from "@/lib/hooks/useTreasury";
import { AbsentBlock, Muted, TreasuryHeader } from "../_shared";

/**
 * Corridors and the destination registry (blueprint §37.1, §38.4), as records.
 *
 * `GET /corridors` answers where capital may go, under what caps, where each
 * corridor sits in its life, and every destination the allowlist holds with
 * the instant it becomes usable. Every stage on this page is the stage the
 * platform recorded; the page does not advance one, and there is no control
 * here that proposes, reviews, signs, activates, suspends or revokes. Those
 * are human acts recorded out of band, and under ADR 0021 nothing this
 * console builds moves a corridor forward.
 *
 * Each registry says whether the process holds it at all. "Not held" is
 * rendered in the platform's words and is a different fact from "held and
 * empty": the first is no registry, the second is an allowlist that admits
 * nothing, and only the second is the safe default doing its job.
 *
 * The twenty-four hour delay is rendered as the platform states it — a
 * `usable_from` instant per destination and an `activation_at` per corridor —
 * rather than computed here from a signature time, so a page cannot show a
 * destination as usable a minute before the registry would say so.
 */
export default function CorridorsPage() {
  const corridors = useCorridors();

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Corridors"
        reads="GET /corridors"
        posture={corridors.data?.posture ?? null}
        meta={<Freshness resource={corridors} name="corridors" />}
      />

      <Panel>
        <PanelHead title="Corridor lifecycle" />
        <PanelBody>
          <ResourceView resource={corridors} loadingRows={3}>
            {(data) => {
              if (!data.corridors.held) {
                return (
                  <AbsentBlock
                    label="registry not held"
                    headline="This process holds no corridor registry."
                    reason={data.corridors.reason}
                    testId="corridors-not-held"
                  />
                );
              }
              const records = data.corridors.records;
              const byStage = new Map<string, number>();
              for (const corridor of records) byStage.set(corridor.stage, (byStage.get(corridor.stage) ?? 0) + 1);
              return (
                <>
                  <KpiRow>
                    <Kpi
                      label="Corridors"
                      value={<span data-testid="corridor-count">{formatCount(records.length)}</span>}
                      note="GET /corridors: corridors.records"
                    />
                    <Kpi
                      label="Active"
                      value={formatCount(byStage.get("active") ?? 0)}
                      note="checked against the signed definition on every intent"
                      tone={(byStage.get("active") ?? 0) > 0 ? "info" : "neutral"}
                    />
                    <Kpi
                      label="Time-delayed"
                      value={formatCount(byStage.get("time_delayed") ?? 0)}
                      note="signed and waiting out the delay"
                    />
                    <Kpi
                      label="Suspended / revoked"
                      value={`${formatCount(byStage.get("suspended") ?? 0)} / ${formatCount(byStage.get("revoked") ?? 0)}`}
                      note="a suspension needs approval to lift; a revocation is permanent"
                      tone={(byStage.get("suspended") ?? 0) + (byStage.get("revoked") ?? 0) > 0 ? "warn" : "neutral"}
                    />
                  </KpiRow>
                  {records.length === 0 ? (
                    <div className="mt-3">
                      <EmptyBlock headline="The registry is held and holds no corridor.">
                        <p>
                          A corridor is where capital may move and under what caps. None is on
                          record, so nothing may move anywhere — the registry&rsquo;s safe default.
                        </p>
                      </EmptyBlock>
                    </div>
                  ) : (
                    <div className="mt-3 flex flex-col gap-3">
                      {records.map((corridor) => (
                        <CorridorCard key={corridor.id} corridor={corridor} />
                      ))}
                    </div>
                  )}
                </>
              );
            }}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Destination registry" />
        <PanelBody flush>
          <ResourceView resource={corridors} loadingRows={3}>
            {(data) =>
              !data.destinations.held ? (
                <div className="p-3">
                  <AbsentBlock
                    label="allowlist not held"
                    headline="This process holds no destination allowlist."
                    reason={data.destinations.reason}
                    testId="destinations-not-held"
                  />
                </div>
              ) : data.destinations.records.length === 0 ? (
                <div className="p-3">
                  <EmptyBlock headline="The allowlist is held, is empty, and permits nothing.">
                    <p>No destination has been proposed. An empty registry is the safe default.</p>
                  </EmptyBlock>
                </div>
              ) : (
                <TableWell maxHeight="420px" label="Every destination the registry holds, in key order">
                  <table className="dt" data-testid="destination-registry">
                    <thead>
                      <tr>
                        <th scope="col">Asset</th>
                        <th scope="col">Address / institution</th>
                        <th scope="col">Status</th>
                        <th scope="col">Proposed</th>
                        <th scope="col">Usable from</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.destinations.records.map((destination) => (
                        <tr key={`${destination.asset}@${destination.address}`} data-testid="destination">
                          <td className="num">{destination.asset}</td>
                          <td className="num">{destination.address}</td>
                          <td>
                            <Chip tone={DESTINATION_TONE[destination.status] ?? "neutral"}>{destination.status}</Chip>
                          </td>
                          <td className="num">
                            {destination.proposed_by} · {formatTimestamp(destination.proposed_at)}
                          </td>
                          <td className="num" data-testid="destination-usable-from">
                            {destination.status === "revoked"
                              ? "never — revoked, and revocation is permanent"
                              : destination.usable_from === null
                                ? `not yet — ${destination.status}, not signed`
                                : formatTimestamp(destination.usable_from)}
                          </td>
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
              A signature on the platform is a record that a person says one exists and where it
              is filed — no key material, nothing this platform could verify or produce (ADR 0009,
              ADR 0021). Usable-from is the registry&rsquo;s own instant; this page does not compute
              it.
            </Muted>
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}

const STAGE_TONE: Record<string, Tone> = {
  proposed: "neutral",
  reviewed: "neutral",
  signed: "info",
  time_delayed: "info",
  active: "ok",
  suspended: "warn",
  revoked: "bad",
};

const DESTINATION_TONE: Record<string, Tone> = {
  proposed: "neutral",
  verified: "info",
  signed: "ok",
  revoked: "bad",
};

function CorridorCard({ corridor }: { corridor: Corridor }) {
  const caps = corridor.caps;
  return (
    <section
      className="flex flex-col gap-2 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2"
      data-testid="corridor"
      data-alert={corridor.stage === "suspended" || corridor.stage === "revoked" ? "true" : undefined}
      aria-label={`corridor ${corridor.id}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="num text-[14px] font-semibold">{corridor.id}</span>
        <Chip tone={STAGE_TONE[corridor.stage] ?? "neutral"}>{corridor.stage}</Chip>
        <Chip tone={corridor.signed ? "ok" : "warn"}>{corridor.signed ? "signature on record" : "unsigned"}</Chip>
        <Chip>{corridor.source_class}</Chip>
        <Chip>{corridor.kind}</Chip>
        {corridor.activation_at !== null ? (
          <span className="num text-[11px] text-[color:var(--color-ink-faint)]" data-testid="corridor-activation-at">
            {corridor.stage === "time_delayed" ? "activates at" : "delay ended"} {formatTimestamp(corridor.activation_at)}
          </span>
        ) : null}
      </div>
      <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">{corridor.purpose}</p>

      <div className="grid gap-4" style={{ gridTemplateColumns: "1fr 1fr" }}>
        <div>
          <span className="eyebrow">route</span>
          <dl className="mt-1">
            <KeyValue label="Source">
              {corridor.source.region} · {corridor.source.currency} · {corridor.source.venue}
            </KeyValue>
            <KeyValue label="Destination">
              {corridor.destination.asset}@{corridor.destination.address}
            </KeyValue>
            <KeyValue label="Proposed">
              {corridor.proposed_by} · {formatTimestamp(corridor.proposed_at)}
            </KeyValue>
            <KeyValue label="Reviewed">
              {corridor.reviewed_by === null ? "—" : `${corridor.reviewed_by} · ${formatTimestamp(corridor.reviewed_at)}`}
            </KeyValue>
          </dl>
        </div>
        <div>
          <span className="eyebrow">caps in force</span>
          <dl className="mt-1" data-testid="corridor-caps">
            <KeyValue label="Per transfer">{formatDecimal(caps.max_per_transfer)}</KeyValue>
            <KeyValue label="Per hour">{formatDecimal(caps.max_per_hour)}</KeyValue>
            <KeyValue label="Per day">{formatDecimal(caps.max_per_day)}</KeyValue>
            <KeyValue label="Cumulative">{formatDecimal(caps.max_cumulative)}</KeyValue>
            <KeyValue label="Minimum interval">{formatSeconds(caps.min_interval_seconds)}</KeyValue>
            <KeyValue label="Permitted hours">
              {String(caps.permitted_hours.start).padStart(2, "0")}:00–
              {String(caps.permitted_hours.end).padStart(2, "0")}:00 UTC
            </KeyValue>
          </dl>
        </div>
      </div>
    </section>
  );
}
