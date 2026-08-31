# These manifests are not what runs

Argo CD applies `infrastructure/helm/qip`. Nothing applies this directory.

`.github/workflows/deploy.yml` used to, with `kubectl apply --server-side -f
rendered/`, and for a while both paths ran — ADR 0017 calls that the migration
window and expected the two to say the same thing. On 2026-08-31 they stopped:
`qip-api-console` and `allow-api-ingress-from-console` were added to the chart
and not here, so the next automatic run would have applied a manifest set that
had never heard of the console route, contesting field ownership with Argo CD
over every object the two share. Nothing caught it, because nothing ever
enforced the equivalence the chart's own header claims — there is no parity
test, and the "proven equivalent by rendered diff" it describes was a step
somebody did once by hand.

So the pipeline's job now stops at the registry: it builds, scans, signs and
attests, then commits the digests to
`infrastructure/helm/qip/values-<env>-images.yaml`. Argo CD is the only writer
to the cluster.

## Why the directory is still here

Twenty-two acceptance checks read these files as the description of the
platform's Kubernetes shape — its network policies, its egress posture, its
workload-identity annotations. Deleting the directory means rewriting all of
them against the chart, which is worth doing and is not this change.

## What that means when you edit something

**A change made only here does not deploy.** It changes what the tests read
and nothing else. If the change should reach the cluster, make it in
`infrastructure/helm/qip/templates/` — the file names correspond one to one —
and let Argo CD sync it.

Treat this directory as a specification the tests check, not as the running
system. The running system is the chart, and the digests it deploys are in
`values-<env>-images.yaml`.
