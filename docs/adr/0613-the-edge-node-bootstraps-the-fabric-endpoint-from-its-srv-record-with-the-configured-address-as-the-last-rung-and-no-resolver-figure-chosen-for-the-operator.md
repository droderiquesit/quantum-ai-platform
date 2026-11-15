# ADR 0613: The edge node bootstraps the fabric endpoint from its SRV record, with the configured address as the last rung and no resolver figure chosen for the operator

- **Status**: Proposed, 2026-11-15, by wave b52 lane 06 for the owner's review.
- **Date**: 2026-11-15
- **Extends**: ADR 0135 (the SRV query, parser and fallback ladder in
  `qip_transport::dns`).
- **Keeps**: ADR 0003, ADR 0109 (no dependency), ADR 0112 (the link's scheme is
  chosen from the configured endpoint).

## Context

ADR 0135 built the client and left no caller, so R12-FABRIC-10016 stayed
PARTIAL: every deployed client still took one literal address. A literal
breaks on broker replacement; DNS loss must not stop trading.

## Decision

1. **One composition root first: `qip-edge-node`.** `fabric_discovery` reads
   `QIP_FABRIC_SRV_RESOLVER`, `QIP_FABRIC_SRV_ZONE`,
   `QIP_FABRIC_SRV_TIMEOUT_MS` and `QIP_FABRIC_SRV_ATTEMPTS`, all four or none.
   The other roots (ledgerd, api, fastbrain, deepbrain) are not wired here.
2. **No figure is chosen.** The timeout and attempt count are required, not
   defaulted. A resolver named without them stops the process.
3. **The configured `QIP_FABRIC_URL` stays required** and is the ladder's last
   rung; the record supplies the authority, the configured scheme is kept.
4. **The rung is a gauge**, `qip_fabric_address_source{source}`, `1` on the rung
   in force, so a region on stale discovery is visible.

## What is not done

No deployment sets the four variables; `manifest_wiring` records why in its
allowlist. The resolver is not read from `resolv.conf`.

## What it costs

An operator who wants discovery must now set four variables together and
choose the timeout and attempt count themselves; naming a resolver without
all four stops the process at start-up, and every figure is a decision the
operator owns. The lookup runs on the start-up path, so a resolver that does
not answer delays start by up to the timeout times the attempts, and that
delay is unmeasured here. The configured
`QIP_FABRIC_URL` must still be supplied, so discovery adds a second source
of the address rather than removing the first. Only `qip-edge-node` is wired:
ledgerd, api, fastbrain and deepbrain still take one literal address, and no
deployment sets the four variables yet.

## What would make this wrong

The decision should be revisited if operators routinely leave discovery
unconfigured because choosing a timeout and attempt count is a burden, in
which case a reviewed default would serve better than a required figure. It
should also be revisited if `qip_fabric_address_source` shows regions sitting
on the configured rung for long periods without anyone noticing, which would
mean the gauge is not enough to expose stale discovery. A start-up delay from
a slow resolver that exceeds what a region can tolerate would argue for
resolving off the start-up path.
