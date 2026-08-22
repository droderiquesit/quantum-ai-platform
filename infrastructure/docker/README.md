# Images

One `Dockerfile`, parameterised by `BINARY`. Built by
`.github/workflows/deploy.yml`, one image per deployable.

```sh
docker build -f infrastructure/docker/Dockerfile --build-arg BINARY=qip-api -t qip-api:dev .
```

The build context is the repository root, not this directory.

## Why `scratch`

There is nothing in the image except the binary. No shell to run, no package
manager to install with, no libc to link against. That is possible here only
because the platform implements its own HTTP, hashing, RNG and numerics and
depends on `serde` and `serde_json` — see
[ADR 0002](../../docs/adr/0002-two-dependencies.md).

It has a cost worth stating: there is no way to `exec` into a running container
to look at something. Debugging is through logs, metrics and a local build, and
an ephemeral debug container if the cluster allows one.

## What is not here

No image signing. The cluster sets `binary_authorization` to
`PROJECT_SINGLETON_POLICY_ENFORCE`, which means it will refuse an unsigned image
— and nothing in this repository signs one. That gap, and what closing it needs,
is in [external dependencies](../../docs/operations/external-dependencies.md).
