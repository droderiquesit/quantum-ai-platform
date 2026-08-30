# Algorik — from Google-issued URLs to algorik.ai

The platform must be usable before `algorik.ai` is connected, and the move to
the real domains must be a configuration change, not a source change. This
runbook is the whole path: what runs on day one, what information unblocks each
step, and the exact order in which the cutover happens so that no step strands
a signed-in user or breaks a redirect mid-flight.

**Standing rule: no URL in this document is real until a deployment returns
it.** The `*.run.app` names below are *shapes*. They are written into
configuration only after `gcloud run deploy` (or the deploy workflow) prints
them, and they are never reported as live before that.

## Phase 0 — where the programme is now

Everything is local. The applications read every URL and identity setting from
validated environment configuration (`frontend/packages/shared-types`), which is the
mechanism that makes the rest of this document configuration-only:

| Key | Local value | After first deploy | After migration |
|---|---|---|---|
| `ALGORIK_SITE_URL` | `http://127.0.0.1:3000` | Cloud Run URL for landing | `https://www.algorik.ai` |
| `ALGORIK_PORTAL_URL` | `http://127.0.0.1:3400` | Cloud Run URL for portal | `https://app.algorik.ai` |
| `ALGORIK_ADMIN_URL` | `http://127.0.0.1:3500` | Cloud Run URL for admin | `https://admin.algorik.ai` |
| `ALGORIK_API_URL` | portal `/api` | Cloud Run URL for api | `https://api.algorik.ai` |

## Phase 1 — first deployment on Google-issued URLs

Blocked on the **GCP bootstrap checkpoint** (below). Once unblocked:

1. Deploy each surface; **record the URLs the deploys print** into the
   environment's configuration and into
   `identity_authorized_domains` in `infrastructure/environments/<env>/terraform.tfvars`.
2. `terraform plan` — review that the only identity change is the domain list —
   then apply with approval. The identity module
   (`infrastructure/terraform/modules/identity/`) is off by default; this is
   the step that turns it on for the environment.
3. Configure the OAuth consent screen and client (the **OAuth checkpoint**),
   with the *recorded* URLs as authorized origins and
   `<portal-url>/api/auth/callback/google` as the exact redirect URI.
   The client id goes into configuration (`ALGORIK_GOOGLE_CLIENT_ID`); the
   client secret goes into **Secret Manager only**. It is not a Terraform
   variable and not an output, because either would put it in state.
4. Smoke: sign-up → verify → sign-in → portal → sign-out against the real
   Identity Platform, then run the auth Playwright project pointed at the
   deployed URL.

## Phase 2 — DNS and TLS for algorik.ai

Blocked on the **domain checkpoint**. Then, in order:

1. **Verify domain ownership** in the target project (Search Console TXT or
   Cloud Domains, whichever the registrar supports).
2. Create the **load balancer** with serverless NEGs per surface and a
   **managed certificate** covering `algorik.ai`, `www`, `app`, `admin`,
   `api`, `partners`, `developers`, `status`. Certificates only provision
   after DNS points at the LB — expect up to an hour of `PROVISIONING`.
3. Cut DNS records over **one hostname at a time**, `www` first (lowest blast
   radius), `app` last (holds sessions). Keep TTLs at 300s during the window.
4. **Mail safety:** before touching the zone, export the existing records. MX,
   SPF, DKIM and DMARC records are copied exactly and never "cleaned up" —
   losing mail during a web migration is the classic self-inflicted outage.

## Phase 3 — application cutover, in dependency order

Each step is a config change; nothing rebuilds:

1. **Authorized domains** — add the algorik.ai hostnames to
   `identity_authorized_domains` (keep the run.app names during transition);
   plan, review, apply.
2. **OAuth redirect URIs** — add `https://app.algorik.ai/api/auth/callback/google`
   *alongside* the run.app URI; remove the old one only after step 5.
3. **Environment URLs** — flip the four `ALGORIK_*_URL` keys per surface.
4. **Cookies** — sessions use `__Host-` cookies, which bind to exact origin:
   nothing to reconfigure, but every user is signed out at cutover by design.
   Announce it; do not fight it with a shared-domain cookie, which would widen
   the cookie to every subdomain including ones that do not need it.
5. **CORS/CSP** — the BFF is same-origin so CORS stays closed; update CSP
   `connect-src`/`img-src` if any absolute run.app URL crept into headers.
6. **Redirects** — permanent 308s: apex → `www`, run.app hostnames → their
   algorik.ai successors (Cloud Run keeps serving both during transition).
7. **PWA deep links** — serve `/.well-known/assetlinks.json` (Android) and
   `/.well-known/apple-app-site-association` (iOS) from the portal at the new
   origin; the installed app's scope follows the manifest, which is
   path-relative and needs no change.
8. **SEO** — canonical URLs on the landing pages flip with `ALGORIK_SITE_URL`;
   submit the new sitemap; leave the run.app landing responding 308, never 200.
9. **Monitoring** — uptime checks against every new hostname *before* DNS
   cutover (they go green as propagation completes, which is the progress
   meter); alert on certificate expiry and on 401-rate at the gateway.
10. **Rollback** — DNS back to the LB-less state is not needed: keep the old
    run.app URLs authorized and configured until one full week green, so
    rollback is "flip the four URL keys back", which takes minutes and loses
    only the sessions created since.

`algorik.com` later: registrar-level 308 to `algorik.ai` apex, plus its own
managed cert if it must terminate rather than redirect at the registrar.

## The checkpoints — exactly what is needed, and why

**GCP bootstrap** (blocks Phase 1): organization id (if any) · billing account
id · folder id (if any) · existing-or-desired project id and naming preference
· approved regions · the GitHub org/repo for workload identity federation ·
identity administrator contact · whether the project exists · whether Identity
Platform is already enabled · whether Firebase was ever initialized on the
project (it changes which console owns the config) · whether Cloud DNS already
hosts `algorik.ai`. Minimum roles for the deploying identity:
`run.admin`, `iam.serviceAccountUser`, `secretmanager.admin` (scoped),
`identityplatform.admin`, `compute.loadBalancerAdmin`, `dns.admin` (Phase 2
only), plus `serviceusage.serviceUsageAdmin` to enable APIs.

**OAuth** (blocks Google sign-in): consent-screen app name and support email ·
authorized domains · privacy-policy and terms URLs (the landing site serves
drafts at `/legal/*` — counsel review before the consent screen goes public) ·
the client id · the exact redirect URIs. **The client secret is never pasted
in chat**: it goes registrar → Secret Manager directly.

**Domain** (blocks Phase 2): current registrar · current DNS provider ·
whether nameservers may move · whether mail uses the domain today · explicit
approval for each record change and for the redirect scheme · the state of
`algorik.com`.

**Mobile stores** (optional, later): Apple Team ID, iOS bundle id, Play
account state, Android application id, signing method, push setup. The
installed PWA needs none of this; store distribution does. Signing keys are
never requested through chat.
