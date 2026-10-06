# Cloud Run Service Inventory

This document records the inventory of services running on Cloud Run, their roles, statefulness classification, and cold-start/latency budgets accepted for each.

## Services

### qip-api (Portal and Operator API)

| Property | Value |
|----------|-------|
| **Role** | Dynamic backend-for-frontend (BFF) and operator control API |
| **Statelessness** | Stateless on the request path; holds in-process Platform (Mutex) for cycle execution |
| **Service Definition** | `backend/crates/apps/qip-api` |
| **Deployment** | Cloud Run via Config Connector (RunService manifest) |
| **Min Instance Count** | 0 (scale-to-zero enabled) |
| **Cold-Start Budget** | 500ms (serves synchronous portal and operator requests) |
| **Latency Budget** | <100ms p50 for portal requests (excluding cycle computation time) |
| **Endpoints** | /api (operator control), /cycle (platform step), /metrics (health), /ready (probe) |
| **Scale Profile** | Burst-tolerant; cold start acceptable on operator-initiated actions |
| **Notes** | Cloud Run placement per ADR 0024. Endpoints tied to cycle steps (SENSE, UNDERSTAND, DISCOVER, REASON, SIMULATE, DECIDE, ACT, LEARN). |

### qip-fastbrain (Fast Reasoning Engine)

| Property | Value |
|----------|-------|
| **Role** | Rapid portfolio rebalancing and decision support |
| **Statelessness** | Stateless per request; caches models in process |
| **Service Definition** | `backend/crates/apps/qip-fastbrain` |
| **Deployment** | Cloud Run via Config Connector (RunService manifest) |
| **Min Instance Count** | 0 (scale-to-zero enabled) |
| **Cold-Start Budget** | 2s (model cache warm-up) |
| **Latency Budget** | <500ms p99 for portfolio decisions |
| **Endpoints** | /decide, /score, /metrics, /ready |
| **Scale Profile** | Burst-driven; fires on portfolio change events |
| **Notes** | Cloud Run placement per ADR 0024. Separate from deepbrain for decision isolation. |

### qip-deepbrain (Model Training and Calibration)

| Property | Value |
|----------|-------|
| **Role** | Offline model training, belief calibration, historical analysis |
| **Statelessness** | Stateless per request; model artifacts are durable |
| **Service Definition** | `backend/crates/apps/qip-deepbrain` |
| **Deployment** | Cloud Run via Config Connector (RunService manifest) |
| **Min Instance Count** | 0 (scale-to-zero enabled) |
| **Cold-Start Budget** | 10s (model load and cache rebuild) |
| **Latency Budget** | <5s for calibration runs (async background work) |
| **Endpoints** | /calibrate, /retrain, /metrics, /ready |
| **Scale Profile** | Scheduled batch jobs; fires after significant events (fills, halts, reconciliation) |
| **Notes** | Cloud Run placement per ADR 0024. Training jobs are not latency-sensitive; cold start is acceptable. |

## Placement Decisions

All three services above were placed on Cloud Run under ADR 0024 with these trade-offs:

1. **Warm Binary Placement**: ADR 0024 standardizes all warm binaries on Cloud Run uniformly, departing from the v2.1 design which reserved Cloud Run for burst stateless endpoints only.
2. **Cold-Start Budgets**: Each service's cold-start budget was set based on its role and latency tolerance:
   - qip-api: short user-facing requests → 500ms budget
   - qip-fastbrain: decision support → 2s budget
   - qip-deepbrain: offline training → 10s budget
3. **Scale-to-Zero**: All three services run with minInstanceCount=0, keeping idle cost off the regional clusters.

## Verification Evidence

- **Service manifests**: `infrastructure/gitops/envs/dev/api.yaml`, `infrastructure/gitops/envs/dev/fastbrain.yaml`, `infrastructure/gitops/envs/dev/deepbrain.yaml`
- **Deployment configuration**: `infrastructure/terraform/modules/cloudrun/main.tf`
- **Scale settings**: Each manifest declares `minInstanceCount: 0` for scale-to-zero behavior
- **Cold-start/latency budgets**: Recorded in this document per the requirement

## Compliance

**API-009 Verification Method**: Inspection

This inventory satisfies the requirement:
> "The service inventory records, for every Cloud Run service, that it is stateless, what role it serves (webhook, control API or other) and the cold-start/latency budget it was accepted against."

All three Cloud Run services are recorded above with:
1. ✓ Statefulness classification (stateless on request path, though qip-api holds in-process Platform for cycle)
2. ✓ Role description (BFF+control API, decision support, training)
3. ✓ Cold-start/latency budgets (500ms, 2s, 10s respectively)

---
**Last Updated**: 2026-10-06
**Status**: Complete (supports API-009 verification)
