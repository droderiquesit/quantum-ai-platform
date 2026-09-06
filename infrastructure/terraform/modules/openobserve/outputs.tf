# What a caller takes from this module: the decision, and the two values the
# manifest must carry if it is to match it. Nothing here reaches Google —
# there is no resource in this module to read an attribute off.

output "access_posture" {
  description = "The posture in force: `authenticated` (ADR 0033) or `anonymous` (ADR 0030). The value a parity test reads to know which invoker it should admit on the RunService."
  value       = var.access_posture
}

output "run_service_ingress" {
  description = "The `ingress` OpenObserve's RunService must declare for this posture. `INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER` under the authenticated posture, because a service still answering on its own run.app URL is a route around Identity-Aware Proxy."
  value       = local.run_service_ingress
}

output "invoker_members" {
  description = "The `roles/run.invoker` members the invoker manifest must name under the authenticated posture: the named principals, and nothing wider — `access_principals` refuses the two anonymous members. Empty under the anonymous posture, where the binding is the manifest's under ADR 0030 and this module names no principal for it."
  value       = local.invoker_members
}
