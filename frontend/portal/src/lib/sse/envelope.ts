import type { SseFrame } from "./parser";

/**
 * The envelope every event on a platform stream carries.
 *
 * The platform sends `stream`, `type`, `sequence`, `cursor`, `event_time`,
 * `ingest_time`, `correlation_id` and `payload`. Two of those are numbers and
 * they answer different questions, which is worth stating because conflating
 * them is the easy mistake:
 *
 * * `sequence` counts deliveries **on this connection**, contiguously from one.
 *   A gap in it means this reader missed something. It restarts at one after
 *   every reconnect, so it is useless for de-duplication.
 * * `cursor` is the position in the platform's own append-only log. It is what
 *   the SSE `id:` field carries and what `Last-Event-ID` resumes from, and it
 *   is the only field a reader may de-duplicate on. It is sparse on a filtered
 *   stream, so a gap in it means nothing at all.
 *
 * The two timestamps are equally load-bearing: `ingest_time - event_time` is
 * the platform's ingestion latency and `now - ingest_time` is the transport's.
 * A feed that is slow and a market that is quiet look identical without them.
 */
export interface StreamEnvelope {
  /** Which channel sent it. */
  readonly stream: string;
  readonly type: string;
  /** Delivery count on the current connection. Contiguous; gaps mean loss. */
  readonly sequence: number | null;
  /** Log position. Stable across reconnects; the de-duplication key. */
  readonly cursor: number | null;
  readonly eventTime: string | null;
  readonly ingestTime: string | null;
  readonly correlationId: string | null;
  /** The event body as the platform sent it. */
  readonly payload: Record<string, unknown>;
  /** The SSE `id:` field — the cursor to resume from. */
  readonly lastEventId: string | null;
  readonly receivedAt: number;
  /** Ingest time minus event time: the platform's own latency. */
  readonly ingestLagMs: number | null;
  /** Arrival here minus ingest time: everything after the platform. */
  readonly transitLagMs: number | null;
  /** The unparsed frame, kept so a malformed event is still inspectable. */
  readonly raw: string;
  readonly malformed: boolean;
}

/** Events the stream sends about itself rather than about the platform. */
export function isStreamNotice(envelope: StreamEnvelope): boolean {
  return envelope.type.startsWith("stream.");
}

function asString(value: unknown): string | null {
  if (typeof value === "string" && value.length > 0) return value;
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  return null;
}

function asNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
}

/** Milliseconds since the epoch for an RFC 3339 stamp or a numeric one. */
function instant(value: string | null): number | null {
  if (value === null) return null;
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    // Decided by magnitude: parts of the platform stamp records in
    // nanoseconds since the epoch, and the API renders others as RFC 3339.
    if (numeric > 1e17) return numeric / 1e6;
    if (numeric > 1e14) return numeric / 1e3;
    if (numeric > 1e11) return numeric;
    return numeric * 1000;
  }
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function malformed(frame: SseFrame, receivedAt: number, type: string): StreamEnvelope {
  return {
    stream: "unknown",
    type: frame.event ?? type,
    sequence: null,
    cursor: frame.id === null ? null : asNumber(frame.id),
    eventTime: null,
    ingestTime: null,
    correlationId: null,
    payload: {},
    lastEventId: frame.id,
    receivedAt,
    ingestLagMs: null,
    transitLagMs: null,
    raw: frame.data,
    malformed: true,
  };
}

export function decodeEnvelope(frame: SseFrame, receivedAt: number): StreamEnvelope {
  let parsed: unknown;
  try {
    parsed = JSON.parse(frame.data);
  } catch {
    return malformed(frame, receivedAt, "stream.unparseable");
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return malformed(frame, receivedAt, "stream.unstructured");
  }

  const record = parsed as Record<string, unknown>;
  const eventTime = asString(record["event_time"] ?? record["eventTime"]);
  const ingestTime = asString(record["ingest_time"] ?? record["ingestTime"]);
  const cursor = asNumber(record["cursor"]) ?? (frame.id === null ? null : asNumber(frame.id));
  const payload = record["payload"];

  const eventAt = instant(eventTime);
  const ingestAt = instant(ingestTime);

  return {
    stream: asString(record["stream"]) ?? "unknown",
    type: asString(record["type"]) ?? frame.event ?? "message",
    sequence: asNumber(record["sequence"]),
    cursor,
    eventTime,
    ingestTime,
    correlationId: asString(record["correlation_id"] ?? record["correlationId"]),
    payload:
      typeof payload === "object" && payload !== null && !Array.isArray(payload)
        ? (payload as Record<string, unknown>)
        : record,
    lastEventId: frame.id ?? (cursor === null ? null : String(cursor)),
    receivedAt,
    ingestLagMs: eventAt !== null && ingestAt !== null ? Math.round(ingestAt - eventAt) : null,
    transitLagMs: ingestAt !== null ? Math.round(receivedAt - ingestAt) : null,
    raw: frame.data,
    malformed: false,
  };
}

/** A one-line description of an event, for a feed row that has no columns. */
export function summarisePayload(envelope: StreamEnvelope): string {
  const detail = envelope.payload["detail"];
  if (typeof detail === "string") return detail;
  const keys = Object.keys(envelope.payload).filter((key) => key !== "stream");
  if (keys.length === 0) return "(no payload)";
  return keys
    .slice(0, 4)
    .map((key) => {
      const value = envelope.payload[key];
      if (value === null) return `${key}=null`;
      if (typeof value === "object") return `${key}={…}`;
      return `${key}=${String(value)}`;
    })
    .join("  ");
}
