# 0095. The portal is reached at its Google-issued run.app URL behind Cloud Run's own IAP, and a custom domain becomes the optional path

Status: **Accepted on delegated authority, 2026-09-21.** Nothing is applied by
this record; it decides where a thing goes and the code follows it.

Narrows ADR 0094 rather than superseding it: that ADR's module survives, gated
off, as the path back to a vanity hostname. It reverses ADR 0094 decision 3 on
that ADR's own stated terms, and it retires the nameserver delegation
`modules/dns-zone` was built to make unnecessary-but-once.

## The ask

The owner will not buy a domain and will not delegate one. Every front door
must therefore have a **Google-provided** hostname, so that no registrar
action and no DNS delegation is ever required in dev.

Three doors in the tree depended on `algorik.ai`: the portal
(`portal.algorik.ai`, ADR 0094), Argo CD and Kargo (`argocd.algorik.ai`,
`kargo.algorik.ai`, `modules/gitops-gateway`), and the zone that would answer
all three (`modules/dns-zone`). All three were applied, or applicable, and
none of them resolved, because replacing nameservers at a registrar is a
person's act in a web form and it had not been performed.

## The finding that decides it: Cloud Run enforces IAP on the service

**Cloud Run supports IAP directly, with no load balancer and no custom
domain.** Google's page is explicit:

> By enabling IAP on Cloud Run directly, you can secure traffic with a single
> click from all ingress paths, including default `run.app` URLs and load
> balancers.

— `cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run`, read
2026-09-21.

A Cloud Run service already has a Google-issued hostname on a Google-managed
certificate: `https://<service>-<project-number>.<region>.run.app`. So a
console door needs no reserved address, no managed certificate, no URL map, no
target proxy, no forwarding rule, no Cloud Armor policy, no DNS zone and no
registrar.

Verified against the provider set this repository actually pins, rather than
taken from the page:

- `terraform providers schema -json` at `hashicorp/google` **6.50.0** and
  `hashicorp/google-beta` **6.50.0**: `iap_enabled` (bool, optional, "Used to
  enable/disable IAP for the service") is present on **google-beta**'s
  `google_cloud_run_v2_service` and **absent** from the GA `google`
  provider's.
- `google_iap_web_cloud_run_service_iam_member`, `_binding` and `_policy` are
  present in **both** providers at 6.50.0, taking `project`, `location` and
  `cloud_run_service_name` — three strings, no resource to own.
- Config Connector's `RunService` CRD has **no** `iapEnabled` field, in
  v1.156.0 (the version `bootstrap/config-connector-operator` pins) or on that
  project's `master`. `grep -c -i iap` over the CRD prints `0` for both.

The third finding is mechanical and says when it changes: Config Connector's
`RunService` is generated from the **GA** provider, and the field is
beta-only. When `iap_enabled` graduates, Config Connector gets `iapEnabled`.

Two further facts from the same page shape everything below:

- **"You cannot configure IAP on both the load balancer and the Cloud Run
  service."** The two doors are alternatives, never layers.
- IAP forwards as its own service agent, so the service still requires
  `roles/run.invoker` for `service-<project-number>@gcp-sa-iap.iam.gserviceaccount.com`
  — the grant `gitops/envs/<env>/invokers.yaml` already carries, unchanged
  from ADR 0094.

## The second finding, from a real apply: the load-balancer door is unbuildable here

`infra.yml` run 71 applied the merged tree against `algorik-dev` and failed:

    Error: Error waiting for Creating SecurityPolicy "qip-dev-portal-iap":
    Quota 'SECURITY_POLICY_RULES' exceeded.  Limit: 0.0 globally.
      with module.portal_edge[0].google_compute_security_policy.edge,
      on modules/iap-edge/main.tf line 130

**Limit 0.0 globally** — not exceeded by one. The project has no Cloud Armor
allowance at all; raising it is a quota request that may not be granted; and a
security policy attaches to a backend service, which exists only because there
is a load balancer. So ADR 0094's door is not merely redundant in dev, it
cannot apply, and while it is in the configuration it takes the rest of the
apply down with it — including the Artifact Registry pull grant four Cloud Run
services were waiting on.

This is treated as a fact about the environment, not a transient failure, and
nothing here is designed around the quota being granted.

## The third finding: there is no Google-provided hostname for a GKE Gateway

Argo CD and Kargo run **in the cluster**. A Cloud Run service gets a `run.app`
name for nothing; a GKE Gateway gets an IP address and nothing else — Google
publishes no DNS name for one — and a Google-managed certificate is issued for
a domain or it is not issued. There is no equivalent to `run.app` here, and
the search for one has to end in a decision rather than in a workaround.

Two workarounds were considered and both are worse than the gap. A
third-party wildcard resolver (`nip.io`, `sslip.io`) puts a control plane's
hostname in somebody else's DNS, which for a controller that can reconcile
arbitrary manifests into the cluster is an outsourced dependency nobody
reviewed. A self-signed certificate trains an operator to click through a
browser warning in front of that same console.

## Decision

1. **The portal is reached at its Google-issued `run.app` URL, behind IAP on
   the Cloud Run service itself.** No load balancer, no address, no
   certificate, no Cloud Armor policy, no zone, no registrar.
   `terraform output console_front_door` prints the URL and the one command
   that admits a person to it.

2. **`modules/iap-edge` is kept, narrowed, and switched off — not retired.**
   It is the path back to a vanity hostname, and the argument it makes about
   why a GKE Gateway cannot front Cloud Run is still true and still worth
   keeping written down. Its `count` stays on `gitops_portal_hostname`, and
   `module.portal_iap_run`'s `count` is the **exact negation** of it rather
   than a flag of its own, because a flag could be set to the combination
   Google refuses and the failure would arrive at apply naming neither door.

3. **The access list becomes per service, reversing ADR 0094 decision 3 on
   its own stated terms.** That decision refused to hold a members list
   because a GKE Gateway's backend service is named by its controller, has no
   Terraform address, and therefore forced a **project-level**
   `roles/iap.httpsResourceAccessor` grant that every IAP resource in the
   project inherits — so a per-resource list could only widen it, never
   narrow it. IAP on Cloud Run is addressable per service, so
   `modules/iap-run` holds a real list: admitting somebody to Argo CD no
   longer admits them to the console. ADR 0094's own reversal condition was
   "the backend service becoming addressable, then the grant narrows and
   decision 3 reverses on its own terms". This is that, by a route it did not
   anticipate. `_member` and not `_binding`: a `_binding` is authoritative and
   would silently revoke an operator granted by hand during an incident.

4. **The list is empty in every environment and stays empty.** An IAM member
   is an account identifier and `.claude/rules/00-enterprise-governance.md`
   refuses one in a committed file. The door comes up admitting nobody, which
   is a posture rather than a failure.

5. **Argo CD and Kargo are reached through the fleet's Connect gateway and a
   port-forward, and publish nothing.** `gitops_gateway_enabled = false`,
   both hostnames empty, and `bootstrap/gateway/overlays/dev/` removed so the
   bootstrap applies nothing and says so. This is how the private cluster was
   already designed to be reached and how `infra.yml` itself reaches it; it is
   the better answer for an operator console rather than merely the cheaper
   one. `modules/gitops-gateway` and `bootstrap/gateway/base/` are kept for
   the day somebody owns a domain.

6. **The portal's ingress becomes `INGRESS_TRAFFIC_ALL`, and the safety is in
   what is absent.** There is no `allUsers` invoker: `invokers.yaml` grants
   `roles/run.invoker` to exactly one principal, IAP's service agent. So if
   IAP were not enabled, the URL would resolve, Google's front end would
   accept the connection, and Cloud Run's own IAM check would refuse every
   request — a browser cannot mint a Google ID token. **The unprotected state
   of this configuration is a console nobody can reach, never a console
   anybody can reach.**

7. **`dns_zone_domain`, `gitops_argocd_hostname`, `gitops_kargo_hostname` and
   `gitops_portal_hostname` may all be empty, and empty means "the
   Google-provided path", not a half-configuration.** Each empty value closes
   a `count` cleanly. Two half-configurations are refused at plan time rather
   than left to fail later: `gitops_gateway_enabled` true with either gateway
   hostname empty (which would order a certificate for `""`), and
   `gitops_portal_hostname` set with no `console_egress_cidr` (ADR 0094's
   precondition, unchanged).

8. **`iapEnabled` is set by `infra.yml`'s `apps` stage, and read back.** It is
   the one field Config Connector's `RunService` cannot carry, for the
   upstream reason above. The step refuses to run where a portal hostname is
   set (Google forbids both doors), says so and exits zero where the service
   does not exist yet, enables IAP where it does, and then **describes the
   service and fails the job unless `iapEnabled` reads true**. A step that
   enables a gate and does not check is a step that reports a protected
   console either way.

## What it costs

- **A `gcloud` call is doing a manifest's job.** It is imperative glue in a
  declarative path, it is the one thing here that is not reconciled, and if
  Config Connector ever issues an update that clears the field it will be
  cleared until the next `apps` dispatch. That risk is stated rather than
  measured: KCC computes an update from a diff against its own spec and
  `iapEnabled` is not in that spec, so it should not be in the request body,
  but this has not been observed in a live cluster and is not claimed as a
  fact. The failure direction is closed either way — see decision 6.
- **Cloud Armor is lost, and is named rather than dropped quietly.** ADR
  0094's door carried a rate-based ban on an admitted session behaving like a
  script. This door has none: IAP decides who may pass and nothing bounds how
  fast an admitted caller may go. The control was never actually applied — the
  quota refused it on the one run that tried — so nothing is being switched
  off, but an environment that does not have it should not be described as
  though it does.
- **A `run.app` URL is ugly and carries the project number.** It is not a
  secret (Cloud Run URLs are enumerable and the service is IAP-gated), but it
  is a hostname nobody will remember, and it changes if the service is
  recreated in another project.
- **First-time IAP enablement in a project with no organisation may need one
  console click.** Google: "you cannot create OAuth clients programmatically…
  we recommend that you first enable IAP on Cloud Run directly from the Google
  Cloud console, or configure a custom OAuth client". The workflow step names
  this in its failure message rather than leaving an operator to find it.
- **Argo CD and Kargo need a terminal.** There is no URL to send somebody;
  reaching them is two commands. For a controller with this authority that is
  a feature, and it is now written down in `infrastructure/gitops/README.md`
  rather than implied by the absence of an overlay.
- **Dev's next apply plans destroys.** Switching the four values to empty
  closes `module.gitops_gateway`, `module.portal_edge` and `module.dns_zone`,
  so a plan will show the gateway address and certificate, the portal edge's
  nine resources and the DNS zone as destroys. None of it is serving anything
  — no name resolves and the Cloud Armor policy never applied — but it is a
  destroy and it needs the owner's eyes on the plan, not an agent's.

## What would make this wrong

- `iap_enabled` graduating to the GA `google` provider, and Config Connector
  gaining `iapEnabled`. Then decision 8's workflow step is deleted and the
  manifest carries the field; nothing else changes.
- A real customer-facing surface shipping, which needs a domain for reasons
  that have nothing to do with IAP. Then `gitops_portal_hostname` is set, the
  Cloud Armor quota is requested, and ADR 0094's door comes back — for the
  portal *instead of* this one, never beside it.
- Google issuing a DNS name for a GKE Gateway, or a managed certificate that
  does not need a domain. Then decision 5 is reconsidered.
- The observation that Config Connector does clear `iapEnabled` on reconcile.
  Then the portal's service moves out of the `RunService` into a
  `google_cloud_run_v2_service` on `google-beta`, where the flag is
  structural — which is a partial reversal of ADR 0036 for one service and
  should be taken deliberately, with an ADR, rather than as a bug fix.

## What was rejected

- **Retiring `modules/iap-edge`.** The custom-domain path is a real future and
  the module's argument about the Gateway is the record of an expensive
  finding. Gated off costs nothing; deleted costs the argument.
- **Keeping both doors, with the Cloud Run one as a fallback.** Google refuses
  IAP on both, so "both" is not a configuration that exists.
- **Making Cloud Armor optional and defaulting it off in `modules/iap-edge`.**
  That would have kept the load-balancer door applicable in a project with no
  quota, and it is the wrong trade: the door still needs a domain, which is
  the requirement being removed, and a security control made optional to clear
  an apply error is a control that will be off everywhere within a month.
- **`nip.io`, `sslip.io` or any third-party wildcard resolver** for Argo CD
  and Kargo. A control plane's hostname in somebody else's DNS.
- **A self-signed certificate on the GKE Gateway.** Teaching an operator to
  click through a TLS warning in front of the most privileged console here.
- **Writing `iapEnabled: true` into `portal.yaml` anyway.** The CRD has no
  such field; every Application syncs with `Validate=true`, so the sync would
  be refused rather than the field silently pruned. That is the right failure
  and still not a working door — and a manifest carrying a field nothing reads
  would read, to the next person, as a gate that is on.
- **Moving the portal's service back into Terraform to get `iap_enabled`
  structurally.** It is the cleanest shape and it is a reversal of ADR 0036
  for one workload; it is named in "what would make this wrong" as the
  deliberate next step if the upstream gap does not close or if reconcile is
  observed to clear the flag. It is not taken as a side effect of a front-door
  change.
- **Granting `allUsers` invoker with `INGRESS_TRAFFIC_ALL`**, the shape most
  documentation shows. IAP guarding a front door with the side entrance open.
