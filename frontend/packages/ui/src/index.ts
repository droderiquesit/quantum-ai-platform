/**
 * Algorik shared UI.
 *
 * The components themselves currently live in the portal, which is where they
 * were written, tested and proven against a live platform. This package is
 * their public surface: every other Algorik surface imports from `@algorik/ui`
 * and never from a relative path into the portal, so the day the files move
 * here, no consumer changes an import.
 *
 * Re-exporting rather than moving is deliberate. The portal carries the only
 * passing behavioural suite in the repository; a path rewrite across it buys
 * tidiness and risks the one surface that is verified. The boundary is what
 * matters, and the boundary is real from here on.
 */
export {
  Chip, StatusChip, Metric, MetricRow, KeyValue, Freshness, StreamControls,
  FEED_TONE, FEED_LABEL, type Tone,
} from "../../../portal/src/components/data/Bits";
export { Panel, PanelHead, PanelBody, TableWell } from "../../../portal/src/components/data/Panel";
export { Kpi, KpiRow, type KpiProps } from "../../../portal/src/components/data/Kpi";
export {
  StateBlock, LoadingBlock, EmptyBlock, UnavailableBlock, MissingEndpointBlock,
  RouteMissingBlock, UnreachableBlock, DeniedBlock, ErrorBlock, ResourceView,
} from "../../../portal/src/components/data/States";
export { SimulatedBanner, SimChip } from "../../../portal/src/components/data/Simulated";
export { EventFeed } from "../../../portal/src/components/data/EventFeed";
export {
  formatCount, formatPercent, formatMicros, formatDecimal, formatDurationMs,
  formatAgo, formatClock, formatUtcDate, formatTimestamp, truncate, directionOf,
  type Direction,
} from "../../../portal/src/lib/format";
