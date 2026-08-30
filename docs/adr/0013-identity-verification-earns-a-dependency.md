# 0013 — Verifying an identity token earns a dependency; issuing sessions does not

**Status:** accepted
**Applies:** ADR 0012's three-part test to the Algorik identity programme.

## The decision

The backend's verification of a Google Identity Platform ID token — JWKS
fetch and cache, RS256 signature check, `iss`/`aud`/`exp`/`nbf` validation,
key rotation — is **admitted as an I/O-edge dependency** when that path is
implemented against a real project. Everything else in the identity programme
is written in-tree: session issuance, cookie handling, CSRF, the provider
abstraction, user provisioning, role resolution, and every frontend package.

Scope: the I/O edge only, in the composition root and the auth service. **Not**
the decision core, which ADR 0009 names and which keeps its two dependencies. A
token verifier cannot change what `NetEdge` deducts.

## Why this one and not the rest

ADR 0012 admits a dependency only where all three conditions hold. Token
verification is the rare case that satisfies them completely:

1. **Getting it wrong is silent.** A verifier that forgets to check `alg`,
   accepts `none`, skips `aud`, caches a rotated key too long, or compares a
   signature non-constant-time does not crash and does not fail a test. It
   quietly authenticates an attacker. The platform learns months later, from
   someone else.
2. **The problem is adversarial and specialist.** JWT is a format whose
   ambiguities are probed deliberately and whose historical vulnerabilities are
   almost entirely implementation defects rather than protocol ones. This is
   the same class of knowledge ADR 0012 cited for TLS.
3. **Mature, widely-audited implementations exist** whose maintenance is
   somebody's job, and whose defects are found by people who do this full time.

Hand-rolling it here would be, in ADR 0012's own words, "the riskier option
wearing the costume of the safer one" — and this repository has already made
that argument once, for TLS, on identical grounds.

## What is refused, and why

The rest of the identity surface fails the test and stays in-tree:

* **Session issuance and cookies.** Fails condition 1: a broken session fails
  loudly, on the first request. The security properties that matter — HTTP-only,
  `Secure`, `SameSite`, short lifetime, rotation, server-side revocation — are
  configuration decisions, not cryptanalysis.
* **CSRF.** A double-submit or origin check is a dozen lines whose failure mode
  is visible in a test.
* **The frontend identity SDK.** The browser needs no vendor SDK to redirect to
  an authorization endpoint and hand a code to its own backend; the
  redirect-and-exchange flow is a URL and a POST. Taking a large transitive
  tree into the browser to avoid writing that is the trade ADR 0012 refuses,
  and it is also how configuration ends up hard-coded in source, which the
  brief separately forbids.
* **Validation, data-fetching, table, router, component-workbench and
  analytics libraries.** None satisfies condition 1. They fail loudly.

## What it costs

**The audit story needs another qualifier.** ADR 0012 already reduced "every
number traces to code in this repository" to a claim about the decision core.
This adds a second edge dependency, and the honest statement is now: the core
is at two, the edge takes TLS and token verification, and each is argued in an
ADR.

**A precedent to argue from.** The next request will cite this. The three
conditions are hard on purpose, and "identity already took one" is not an
argument — the refusals listed above are part of this decision, not an
afterthought to it.

**Nothing is admitted before it is needed.** Until a real Identity Platform
project exists, the verifier has no configuration to verify against. So the
programme ships the provider abstraction and a local development adapter first,
with the verification seam typed and tested; the dependency lands with the
Google adapter, and `check-dependencies.sh` stays green until then.

## What would make this wrong

* **If token verification is terminated elsewhere** — an API gateway or IAP
  that validates the token and forwards a trusted assertion, which is the
  standard Google answer — then the platform never parses a JWT, condition 3 is
  satisfied by the gateway, and this ADR should be reduced to nothing. This is
  the likely outcome for the admin surface, which sits behind IAP by design.
* **If the dependency is ever found to have influenced a number the core
  computed**, the separation is not real and the answer is to widen the core
  rather than patch the instance — ADR 0009's reversal condition, unchanged.
