# ADR 0031 — A vendored workload may take a secret as an environment value

- **Status:** accepted
- **Date:** 2026-09-04
- **Amends:** `.claude/rules/01-security-and-safety.md`'s "never as environment
  variables", for vendored workloads only
- **Answers:** the question `modules/cloudrun/variables.tf` left open, and ADR
  0028 decision 3's boundary between a vendored image and a built one

## The decision

`modules/cloudrun` gains a `secret_env` input. It projects a Secret Manager
version into an environment variable through Cloud Run's own
`value_source.secret_key_ref`, and it is **refused unless
`image_source == "vendored"`**. The platform's own binaries cannot reach it,
in any environment, by any tfvars edit.

`module.openobserve` uses it for `ZO_ROOT_USER_EMAIL` and
`ZO_ROOT_USER_PASSWORD`, and its file mounts are removed rather than left
beside it.

## Why the file mount could not work

Not an argument from preference. Measured against the image this catalogue
pins, `openobserve@sha256:88fb692a…`:

- Its config is `Entrypoint: None`, `Cmd: ["/openobserve"]`.
- **It contains no shell.** 1,748 files, and nothing executable under `bin/`
  or `usr/bin/` — no `sh`, no `bash`, no busybox. So there is no entrypoint
  wrapper that could read a mounted file and `exec`, because there is nothing
  to run a wrapper *with*, and a Cloud Run `command` override can only invoke
  `/openobserve` itself.
- Every `ZO_*` symbol in the binary that ends `_FILE` names a cache, compact
  or data path. **There is no `_FILE` indirection for the credential** —
  `ZO_ROOT_USER_EMAIL` and `ZO_ROOT_USER_PASSWORD` are read as environment
  values and nothing else.

So the mount was correct by the rule and inert in fact: the file was
projected at 0400, the `_FILE` variable held its path, and the process opened
neither. `variables.tf` said so already — "a mount here is not evidence the
credential arrived" — and this record is what that sentence was waiting for.
A control that reads as protection and is not is the defect this repository
names most often; keeping the mount beside a working env var would have been
a second one.

## What it costs

The rule's reason is real and does not stop being real here: an environment
value is readable from `/proc/<pid>/environ`, by every child process, and in
every crash dump.

What is different for *this* workload, and why the exception is drawn at
`vendored` rather than waived generally:

- **No child processes and no shell.** The container runs one static binary
  with no `sh` to spawn anything and no tooling to read `/proc` with. The
  usual path from "value in the environment" to "value read by something
  else" does not exist in this image.
- **The value is still not in the repository, in a plan, or in state.** Cloud
  Run resolves `secret_key_ref` at container start. Terraform carries the
  secret's *name*, never its content, so `terraform show` and the state file
  hold nothing.
- **A crash dump remains the live exposure**, and it is not closed by
  anything here. Named, not solved.

The narrower cost is that this platform now has two credential paths instead
of one. That is why `secret_env` refuses a built workload: `qip_core::secret`
exists, every binary this platform compiles uses it, and the day someone
reaches for the easier input on a built workload is the day the rule stops
meaning anything. The refusal is a precondition, not a convention.

## The alternative, and why it was not taken

Build a wrapper image: a base with a shell, an entrypoint that reads the
mounted files and execs `/openobserve`. That keeps the rule intact and is the
technically cleaner answer.

It was not taken because it changes what the image *is*. ADR 0028 decision 3
draws vendored and built apart precisely so that "which foreign code runs
here" is answered by the git history of `vendored-images.txt`; an image this
platform assembles is a built image and belongs in the build→sign→attest
pipeline, with its own base to track and patch. Trading one narrow, refused-
by-default input for a new image to maintain is the worse trade at this size.

Should a second vendored workload ever need this, that is the moment to
reconsider: one exception is a decision, two is a pattern, and a pattern
should be a wrapper base image the platform builds once.

## What would make this wrong

**A vendored workload that has a `_FILE` option and takes this path anyway.**
The exception is for binaries that cannot read a file, not for binaries whose
operator found the env var easier. A reviewer seeing `secret_env` should
check the image for `_FILE` support before accepting it, the way this record
checked.

**A future OpenObserve release gaining `_FILE` support**, which would make the
mount work and this exception unnecessary. Worth checking at the next digest
bump, which ADR 0028's correction already makes an explicit act.

And the standing one: if this ever appears on a workload where
`image_source` is `built`, the precondition has been removed and the rule has
been lost rather than amended.
