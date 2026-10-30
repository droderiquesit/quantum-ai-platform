# Environment variables the deployed workloads read

Companion to [the binaries runbook](binaries.md), which documents the keys each
binary reads from its own source. This page covers the other direction: every
`QIP_*` variable that `infrastructure/terraform/catalogue.tf` (Cloud Run) or
`infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl`
(the execution node) sets, so that no deployed workload is configured by a
value an operator cannot look up. The acceptance test
`gitops.rs::every_environment_variable_a_workload_reads_is_documented` fails
when either file sets a variable that no page under `docs/ops` names, which is
the guard against configuration that exists only in Terraform.

A variable here is set by Terraform from a reviewed input. None is an
emergency switch: changing one is a Terraform change, planned and shown before
it is applied, and no runbook asks for a hand edit on a running service.

## Cloud Run workloads (`catalogue.tf`)

| Variable | Meaning |
|---|---|
| `QIP_DEPLOYMENT` | The deployment name (`var.environment`). Every composition root reads it so it can refuse a seed-derived signing key or a plaintext link anywhere but `local`. A binary refuses to start without it. |
| `QIP_ARTEFACT_SIGNER` | Set to `kms` when an artefact trust bundle is configured: the API signs grants with a Cloud KMS key rather than a reproducible one. |
| `QIP_TRUST_BUNDLE_PATH` | Path of the committed trust bundle baked into the image; public keys only. |
| `QIP_GOOGLE_TLS_ROOTS_PATH` | Path of the Google TLS roots baked into the image, used to verify the KMS endpoint. |
| `QIP_KMS_KEY_VERSIONS_PATH` | Path of the mounted `kms-key-versions.json`, the map from signer to Cloud KMS key version. |
| `QIP_TOKEN_OPERATOR_FILE`, `QIP_TOKEN_ANALYST_FILE`, `QIP_TOKEN_VIEWER_FILE`, `QIP_TOKEN_MONITOR_FILE` | The `_FILE` form of each bearer credential: a path to a Secret Manager mount, resolved by `qip_core::secret`. The credential itself is never an environment value. |
| `QIP_ALPACA_API_KEY_ID_FILE`, `QIP_ALPACA_API_SECRET_KEY_FILE` | The `_FILE` form of the two halves of the Alpaca market-data credential: paths to Secret Manager mounts. Terraform creates the empty containers; a person writes the version out of band (ADR 0040). A mount is not a registration: the source stays refused by the licensing and registration gates until a record is committed. |
| `QIP_QUANTUM_PROVIDER` | The quantum provider the compute router attaches. Unset runs the classical router. Dev and test name the simulator; stage and prod leave it unset. A simulator's answer validates a formulation and is never evidence of advantage. |
| `QIP_QUANTUM_INSPIRED` | Whether the quantum-inspired rung of the compute router is enabled. Unset leaves it off. |

## Execution node (`startup.sh.tftpl`)

| Variable | Meaning |
|---|---|
| `QIP_TLS_MODE` | Always `cas` on the node: the peer link's certificate is issued by Certificate Authority Service. No plaintext fallback exists to select. |
| `QIP_CAS_CA_POOL` | The CAS CA pool the node requests its certificate from. |
| `QIP_CAS_CERTIFICATE_TEMPLATE` | The CAS certificate template the request names. |
| `QIP_CAS_LIFETIME_SECS` | Certificate lifetime in seconds; the node renews at two thirds of it and holds the key in memory only. |
| `QIP_TLS_TRUST_ROOTS_PATH` | Path of the PEM file of trust roots the peer link verifies against, written under the node's configuration directory. |
| `QIP_FENCING_LEASE` | Path of the fencing lease file. Unset is a node with no standby, announced at start. A standby with no lease is refused at plan time. |
| `QIP_NODE_ROLE` | `primary`, `standby` or `unpaired`. Required on every deployed node. |
| `QIP_NODE_NAME` | This process's name in the lease; distinct between a primary and its standby. |
| `QIP_PEER_MESH_LISTEN` | The `ip:port` the peer Reflex Mesh link (ADR 0116) accepts peers on, `0.0.0.0:<listen_port>` from the node entry's `peer_mesh`. Written only when `peer_mesh` is declared; unset, the node announces that it coordinates with no other cell directly and the peer saga runs nowhere. Null in every committed environment (decision 5 of 2026-10-25: a peer's stable address is billed infrastructure). |
| `QIP_PEER_MESH_PEERS` | The peers as `name=ip:port,…`, each name the DNS name the peer's workload certificate carries. The binary refuses this without `QIP_PEER_MESH_LISTEN` and the reverse, a peer named as this node, a hostname, or a public address the module already refused at plan. |
| `QIP_PEER_MESH_CLOCK_BOUND_MS` | The widest clock disagreement, 1 to 1000 ms, at which the node joins an epoch with a peer. Written only when the entry states one; absent is the binary's own 5 ms, tight enough for one metro and refusing a cross-region pair until a wider bound is stated. |
| `QIP_POLICY_ROLLBACK_PATH` | Path of the operator's policy-rollback request file. Unset is a node with no rollback control. |
| `QIP_EXECUTION_NODE_SHADOW` | A marker, not a control: the binary does not read it. What keeps the node off a venue session is the absence of a venue egress rule and a venue-credential binding, both decided in Terraform. |
