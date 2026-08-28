"use client";

import { isStreamNotice, summarisePayload, type StreamEnvelope } from "@/lib/sse/envelope";
import { formatClock, formatDurationMs } from "@/lib/format";
import type { EventStream } from "@/lib/hooks/useEventStream";
import { useNow } from "@/lib/hooks/useNow";
import { Chip } from "./Bits";
import { TableWell } from "./Panel";
import { LoadingBlock, StateBlock } from "./States";

/**
 * A stream's events, newest first, with the two latencies that say whether the
 * feed is healthy.
 *
 * The disconnected and stale states are as detailed as the connected one on
 * purpose. A dashboard that renders a stale feed the same as a live one is
 * worse than one that renders nothing, because it invites a decision on a
 * number nobody has checked the age of.
 */
export function EventFeed({
  stream,
  channel,
  maxHeight = "420px",
  renderRow,
}: {
  stream: EventStream;
  channel: string;
  maxHeight?: string;
  renderRow?: (envelope: StreamEnvelope) => React.ReactNode;
}) {
  const now = useNow();

  if (stream.state === "connecting" && stream.events.length === 0) {
    return <LoadingBlock rows={4} label={`connecting to /api/v1/stream/${channel}`} />;
  }

  if (stream.events.length === 0 && (stream.state === "reconnecting" || stream.state === "error")) {
    return (
      <StateBlock
        tone="bad"
        label="disconnected"
        headline={`The ${channel} stream is not connected.`}
        action={
          <button type="button" className="btn" onClick={stream.reconnect}>
            Reconnect now
          </button>
        }
      >
        <p>{stream.error ?? "The connection dropped and has not been re-established."}</p>
        <p className="mt-1.5 num text-[color:var(--color-ink-faint)]">
          attempt {stream.attempts}
          {stream.retryAt !== null && now !== null
            ? ` · next in ${Math.max(0, Math.round((stream.retryAt - now) / 1000))}s`
            : ""}
          {stream.cursor !== null ? ` · will resume from cursor ${stream.cursor}` : ""}
        </p>
      </StateBlock>
    );
  }

  if (stream.events.length === 0 && stream.state === "paused") {
    return (
      <StateBlock
        tone="neutral"
        label="paused"
        headline={`The ${channel} stream is paused.`}
        action={
          <button type="button" className="btn" onClick={() => stream.setPaused(false)}>
            Resume
          </button>
        }
      >
        <p>Nothing is being received. Resume to reconnect from the last cursor.</p>
      </StateBlock>
    );
  }

  if (stream.events.length === 0) {
    return (
      <StateBlock
        tone="neutral"
        label="connected, no events"
        headline={`The ${channel} stream is open and has sent nothing yet.`}
        compact
      >
        <p>
          The platform opens a stream at the live edge with a bounded backlog. An empty feed here
          means the platform has recorded nothing on this channel, not that the connection failed.
        </p>
      </StateBlock>
    );
  }

  return (
    <>
      {stream.state === "stale" ? (
        <p
          className="border-b border-[color:var(--color-warn)]/40 bg-[color:var(--color-warn)]/10 px-3 py-1.5 text-[11.5px] text-[color:var(--color-warn)]"
          role="alert"
        >
          Stale — not even a heartbeat has arrived for{" "}
          {stream.lastActivityAt !== null && now !== null
            ? formatDurationMs(now - stream.lastActivityAt)
            : "some time"}
          . Everything below predates that.
        </p>
      ) : null}
      {stream.state === "reconnecting" || stream.state === "error" ? (
        <p
          className="border-b border-[color:var(--color-down)]/40 bg-[color:var(--color-down)]/10 px-3 py-1.5 text-[11.5px] text-[color:var(--color-down)]"
          role="alert"
        >
          Disconnected — reconnecting (attempt {stream.attempts}). Rows below are the last received
          and are not current.
        </p>
      ) : null}
      {stream.gaps.length > 0 ? (
        <p
          className="border-b border-[color:var(--color-warn)]/40 bg-[color:var(--color-warn)]/10 px-3 py-1.5 text-[11.5px] text-[color:var(--color-warn)]"
          role="alert"
        >
          Sequence gap: expected {stream.gaps[0]?.expected}, received {stream.gaps[0]?.received}.
          Events were lost; reconcile over REST before trusting this feed.
        </p>
      ) : null}

      <TableWell maxHeight={maxHeight} label={`${channel} event feed`}>
        <table className="dt">
          <thead>
            <tr>
              <th scope="col">Received</th>
              <th scope="col" className="n">
                Seq
              </th>
              <th scope="col" className="n">
                Cursor
              </th>
              <th scope="col">Type</th>
              <th scope="col" className="n">
                Ingest lag
              </th>
              <th scope="col" className="n">
                Transit
              </th>
              <th scope="col">Detail</th>
              <th scope="col">Correlation</th>
            </tr>
          </thead>
          <tbody>
            {stream.events.map((envelope, index) => (
              <tr
                key={`${envelope.cursor ?? "n"}-${envelope.sequence ?? index}-${envelope.receivedAt}`}
                data-alert={envelope.malformed || isStreamNotice(envelope) ? "true" : undefined}
              >
                <td className="num text-[color:var(--color-ink-dim)]">
                  {formatClock(envelope.receivedAt)}
                </td>
                <td className="n">{envelope.sequence ?? "—"}</td>
                <td className="n">{envelope.cursor ?? "—"}</td>
                <td>
                  <Chip tone={isStreamNotice(envelope) ? "warn" : "info"}>{envelope.type}</Chip>
                </td>
                <td className="n" title="ingest_time − event_time: the platform's own latency">
                  {formatDurationMs(envelope.ingestLagMs)}
                </td>
                <td className="n" title="arrival here − ingest_time: everything after the platform">
                  {formatDurationMs(envelope.transitLagMs)}
                </td>
                <td className="max-w-[520px] truncate text-[11.5px] text-[color:var(--color-ink-dim)]">
                  {renderRow ? renderRow(envelope) : summarisePayload(envelope)}
                </td>
                <td className="num text-[10px] text-[color:var(--color-ink-faint)]">
                  {envelope.correlationId ?? "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWell>
    </>
  );
}
