# Execution region selection: measured findings

**Version:** 2026-10-06  
**Status:** v2.1 Framework – Three regions (Americas, Europe, APAC)

## Selection methodology

ADR 0008 calls for the Reflex Cell architecture. ADR 0099 governs regional placement and the cost ceiling (C8) that gates deployment. The exact GCP region for each execution region is chosen by measurement across six dimensions:

1. **Venue RTT** — Round-trip time from the execution region to the primary trading venue(s)
2. **Market-data RTT** — Round-trip time from the execution region to market-data sources
3. **Legal residency** — Data residency requirements and regulatory implications
4. **Available machine series** — Availability of C4D/C4-class compute in the region
5. **Interconnect options** — Dedicated GCP Interconnect or partner circuits available
6. **Counterparty proximity** — Colocation or proximity to settlement and clearing counterparties

The three v2.1 execution regions span three geographical zones for independent failure domains:

| Logical Region | Geographical Zone | GCP Region | Status |
| --- | --- | --- | --- |
| Americas | US East/Central | _Not yet measured_ | Requires selection |
| Europe | EU Central | _Not yet measured_ | Requires selection |
| APAC | Asia-Pacific | _Not yet measured_ | Requires selection |

## Measurement data

### Americas execution region

**Status:** Selection pending measurement.

The Americas region must serve US-listed equities and futures (CME Group, NYSE, NASDAQ). Primary venues:
- CME Group (Chicago)
- NYSE/NASDAQ (New York)
- CBOT, NYMEX co-located with CME

**Candidate GCP regions:** `us-central1`, `us-east4`, `us-south1`

**Data to be collected:**
- RTT to CME Group primary data center (Chicago, IL)
- RTT to NYSE/NASDAQ primary data centers (New York, NY)
- RTT to market-data feeds (Bloomberg, CME, etc.)
- Machine availability (C4/C4D) in each candidate region
- Interconnect costs and lead times
- Settlement venue proximity (DTCC, NSCC)

### Europe execution region

**Status:** Selection pending measurement.

The Europe region must serve EU-listed equities and derivatives. Primary venues:
- Eurex (Frankfurt)
- Euronext (multiple locations: Paris, Amsterdam, Lisbon)
- ICE Europe (London)

**Candidate GCP regions:** `europe-west1`, `europe-west3`, `europe-west4`

**Data to be collected:**
- RTT to Eurex primary data center (Frankfurt, Germany)
- RTT to Euronext hubs (Paris, Amsterdam)
- RTT to market-data feeds
- Machine availability (C4/C4D) in each candidate region
- Interconnect costs and lead times
- Settlement venue proximity (Euroclear, LCH)

### APAC execution region

**Status:** Selection pending measurement.

The APAC region must serve APAC-listed equities and derivatives. Primary venues:
- Japan Exchange Group (Tokyo)
- Hong Kong Exchanges and Clearing (Hong Kong)
- Singapore Exchange (Singapore)

**Candidate GCP regions:** `asia-northeast1`, `asia-east1`, `asia-southeast1`

**Data to be collected:**
- RTT to Japan Exchange Group primary data center (Tokyo, Japan)
- RTT to Hong Kong Exchanges and Clearing (Hong Kong)
- RTT to Singapore Exchange (Singapore)
- RTT to market-data feeds
- Machine availability (C4/C4D) in each candidate region
- Interconnect costs and lead times
- Settlement venue proximity (JASDEC, Hong Kong Central Clearing, CDP)

## Selection criteria and precedent

The edge-cell model (ADR 0008, documented in `docs/operations/deploying-an-edge-cell.md`) established that:

- **Distance matters:** Three edge cells are placed in regions GCP has no presence (`us-central1` is Council Bluffs, Iowa rather than Chicago). The architectural gap was documented as "several milliseconds of round trip that a cell whose whole argument is source-adjacency cannot spend."
- **Colocation/Interconnect is the answer:** For regions GCP does not serve natively, partner Interconnect provides the physical connection. `modules/connectivity` reserves this capacity.
- **Tradeoffs are real:** Some venues simply cannot be co-located with GCP infrastructure. The selection process must document which tradeoffs are accepted and why.

## Recording the final selection

Once the measurements above are collected and a region is selected for each execution zone, the final selection must be recorded by:

1. **Updating this file** with the selected regions and the measured RTT/residency/machine/Interconnect/proximity findings
2. **Adding a comment to the Terraform configuration** in `infrastructure/environments/prod/terraform.tfvars` explaining the selection rationale
3. **Updating `infrastructure/terraform/modules/connectivity/main.tf`** with any partner Interconnect configurations required
4. **Recording in `DELIVERY-STATUS.md`** with the date of selection and the measurement source

See `infrastructure/terraform/modules/execution-node/README.md` for the machine-type validation and Regional computing resources section.

## Current blockers

- **C8 (deployment cost ceiling):** No execution region is deployed until the owner approves the cost ceiling for three regional Reflex cells, three regional GKE clusters, and their supporting infrastructure.
- **Venue access:** Some venues require legal entity presence or sponsorship before latency measurements can be performed.
- **Interconnect provisioning:** Partner Interconnect circuits can have 2–6 week lead times; selection must account for procurement timeline.

See `.claude/rules/01-security-and-safety.md` and `.claude/rules/10-product-direction.md` for the paper-trading boundary and deployment prerequisites.
