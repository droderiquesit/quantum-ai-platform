/**
 * The typed client for the Algorik platform surface, and the hooks over it.
 *
 * Every surface reaches the platform through this package and through the BFF
 * it calls — never directly. That is what keeps the platform's own address,
 * its credential and its internal shape out of every browser.
 *
 * `useResource` is the reason no data-fetching library is admitted here: it
 * distinguishes the four failure states that matter on a trading surface —
 * the route is absent, the credential was refused, the platform is
 * unreachable, and the subsystem answered "nothing, and here is why" — which a
 * generic query cache collapses into one `error`.
 */
export {
  platform, request, isOk, describeOutcome, GATEWAY_PREFIX, GATEWAY_HEADER,
  type ApiOutcome, type ApiResponse, type GatewayDisposition,
} from "../../../portal/src/lib/api/client";
export { REST, STREAM_CHANNELS, NOT_YET_SERVED, type EndpointSpec, type MissingEndpoint, type StreamChannel } from "../../../portal/src/lib/api/endpoints";
export { useResource, type Resource, type UseResourceOptions } from "../../../portal/src/lib/hooks/useResource";
export { useEventStream, backoffFor, type EventStream, type UseEventStreamOptions, type StreamGap } from "../../../portal/src/lib/hooks/useEventStream";
export { useSeries, describeWindow, type Series } from "../../../portal/src/lib/hooks/useSeries";
export { useNow } from "../../../portal/src/lib/hooks/useNow";
export { connections, useConnections, type FeedState } from "../../../portal/src/lib/hooks/connections";
