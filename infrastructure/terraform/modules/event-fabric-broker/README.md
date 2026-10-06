# Event Fabric Broker Module

Provisions event fabric broker instances on GCE. Implements FABRIC-087 (bootstrap via SRV records) and FABRIC-089 (stable internal addresses, no public endpoint).

## Architecture

Per ADR 0100 §3, the first build is RF1 with one broker per region, running on a dedicated GCE VM with fsync durability. This module creates:

- **Instance Template**: `google_compute_instance_template` with no external IP (FABRIC-089)
- **Internal DNS A Record**: For broker discovery (FABRIC-089)
- **DNS SRV Record**: For client bootstrap (FABRIC-087: `_qip-event-fabric._tcp`)

## Requirements

- FABRIC-086 (Broker durability/replication) depends on consensus (C2), so `enabled=false` is the default
- Brokers must have no public IP (FABRIC-089 invariant)
- Clients must discover brokers via DNS SRV records (FABRIC-087 contract)

## Inputs

| Variable | Type | Default | Required | Notes |
|----------|------|---------|----------|-------|
| `enabled` | bool | false | No | ADR 0100: false until C2 consensus record exists (FABRIC-086) |
| `region` | string | - | Yes | GCP region for deployment |
| `project_id` | string | - | Yes | GCP project ID |
| `network_id` | string | - | Yes | VPC network (must exist) |
| `subnet_id` | string | - | Yes | VPC subnet (must exist) |
| `boot_image` | string | - | Yes | Must be self-link, not family (security) |
| `service_account_email` | string | - | Yes | Workload Identity account |
| `dns_zone_name` | string | - | Yes | e.g., `event-fabric.internal` |
| `managed_zone` | string | - | Yes | Cloud DNS managed zone resource |
| `broker_internal_addresses` | list(string) | - | Yes | Internal IPs for brokers (FABRIC-089) |
| `qip_events_config` | string | - | Yes | Base64-encoded qip-events config |
| `qip_fabricd_config` | string | - | Yes | Base64-encoded qip-fabricd config |

## Outputs

- `instance_template_self_link`: For instance group creation
- `broker_dns_name`: Internal DNS name for clients (FABRIC-089)
- `broker_srv_record`: SRV record for bootstrap (FABRIC-087)

## Testing

```
terraform test
```

Tests verify:
- FABRIC-087: SRV records created and resolve correctly
- FABRIC-089: No external IP in instance template
- FABRIC-089: Internal-only network configuration

## Notes

- No external address = FABRIC-089 requirement (no public endpoint)
- Startup script verifies no external IP assigned (defense in depth)
- DNS records enable client bootstrap without hardcoded IPs (FABRIC-087)
- ADR 0100 §8's first vertical slice runs with `enabled=false` on a configured address
