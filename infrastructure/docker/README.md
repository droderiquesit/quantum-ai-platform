# Images

Three image definitions.

| File | What it builds | Built by |
|---|---|---|
| `Dockerfile` | Every platform binary, one image each, selected by `BINARY` | `.github/workflows/deploy.yml` |
| `portal.Dockerfile` | The authenticated console | `scripts/deploy-frontends.sh` |
| `landing.Dockerfile` | The public landing application | `scripts/deploy-frontends.sh` |

```sh
docker build -f infrastructure/docker/Dockerfile --build-arg BINARY=qip-api -t qip-api:dev .
```

The platform build's context is the repository root, not this directory. The two
front-end builds take `frontend/` and `frontend/landing/` respectively, and the
script copies the file into the context root because Cloud Build reads a
`Dockerfile` from there.

## Why `scratch`

There is nothing in a platform image except the binary. No shell to run, no
package manager to install with, no libc to link against. That is possible only
because the platform implements its own HTTP, hashing, RNG and numerics and
depends on `serde` and `serde_json` — see
[ADR 0002](../../docs/adr/0002-two-dependencies.md).

It has a cost worth stating: there is no way to `exec` into a running container
to look at something. Debugging is through logs, metrics and a local build.
There is no ephemeral debug container either, because there is no cluster —
Kubernetes was retired under ADR 0024.

## Every base is pinned by digest

Five `FROM` lines name an upstream image and every one carries an `@sha256:`
index digest beside its tag. A tag is a name its owner can move after the review
that approved it, so a policy that trusts a tag trusts whoever can push it;
`.claude/rules/domains/infrastructure.md` says so and
`infrastructure/egress/vendored-images.txt` has always applied it to the two
third-party images. These carry the same discipline. Moving a base is an edit to
a digest, which is a line a reviewer sees.

Nothing yet refuses a tag coming back. The acceptance suite's
`the_image_runs_as_a_non_root_user_on_an_empty_filesystem` reads `Dockerfile`
and checks `scratch`, `USER` and `--locked`; it checks no digest and never opens
the two front-end files. A test enumerating this directory and refusing any
upstream `FROM` without a 64-character lowercase digest is outstanding work, and
until it exists this section describes a convention rather than a control.

The one exception to the pinning is stated in `Dockerfile` where it happens:
`apk add musl-dev` takes whatever revision alpine serves. The digest freezes the
alpine minor and the repository list, not the package revision, and that package
is in the builder rather than in anything that ships.

## What is not here

**Signing for the two front ends.** `deploy.yml` signs every platform image and
creates a Binary Authorization attestation for it, and
`modules/binaryauthorization` sets the project policy to `REQUIRE_ATTESTATION`
with `ENFORCED_BLOCK_AND_AUDIT_LOG`. `scripts/deploy-frontends.sh` does neither:
it builds the portal and the landing through Cloud Build and deploys them by
tag, with no signing step and no attestation, into the same project. Closing
that is a change to the script and to the deploy identity, not to these files.

**A `.dockerignore`.** There is none anywhere in the repository, so every build
uploads the whole context — `.git`, and `backend/target` on any machine that has
one. The images are still the right bytes, because each `COPY` names what it
wants; the cost is upload time, on four matrix builds.
