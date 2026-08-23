# The gates, runnable locally.
#
# Every target here has a counterpart job in .github/workflows/ci.yml, and the
# command is the same command. That is the whole point: a contributor who runs
# `make` and sees green should not then be surprised by CI, and a CI failure
# should be reproducible in one line rather than by reading YAML.
#
# Ordered cheapest-first, same as CI. A formatting mistake fails in seconds
# instead of after the test suite.
#
# `make` with no target runs the gates that need no network. `make all` adds
# the ones that do (the advisory database, the Terraform provider schema).

SHELL := /usr/bin/env bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := check

# CI fails on any warning. Locally it does too, so the two agree.
export RUSTFLAGS ?= -D warnings
export CARGO_TERM_COLOR ?= always

TERRAFORM_DIR := infrastructure/terraform
DEV_TFVARS := ../environments/development/terraform.tfvars

.PHONY: check all fmt fmt-check lint test test-release build release \
        deps secrets audit sbom tf-fmt tf-validate infra e2e acceptance \
        count doc clean help

# ---------------------------------------------------------------------------
# The offline gate set. This is what `make` runs, and what must pass before a
# push. Nothing here reaches the network beyond the toolchain.
# ---------------------------------------------------------------------------

check: fmt-check lint test deps secrets
	@echo "offline gates: all passed"

# Everything, including the gates that need a network.
all: check build audit sbom infra
	@echo "all gates: all passed"

# ---------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --workspace --all-targets --all-features

test:
	cargo test --workspace --all-features

# The same suite, reported honestly.
#
# Summing `test result: ok. N passed` misses a failing binary entirely — it
# prints `FAILED` instead and contributes nothing — so the count comes back
# lower and still looks clean. This target counts both columns and exits
# non-zero if anything failed. Quote its output, not a grep.
count:
	./scripts/count-tests.sh

# A green debug test suite that cannot be released is not a green build. The
# release profile also catches the optimisation-dependent problems debug hides.
build release:
	cargo build --workspace --release --locked

# The two suites worth naming separately, because they are the ones that answer
# "does the platform work" rather than "does this function work".
#
# `e2e` is the single automated run through all seven layers, from a discovered
# source to a realised fill to a counterfactual to a model update.
e2e:
	cargo test -p qip-acceptance --test e2e -- --nocapture

acceptance:
	cargo test -p qip-acceptance

doc:
	cargo doc --workspace --no-deps

# ---------------------------------------------------------------------------
# Policy
# ---------------------------------------------------------------------------

# The dependency policy. See docs/adr/0009-tiered-dependency-policy.md: the
# decision core keeps serde and serde_json and nothing else.
deps:
	./scripts/check-dependencies.sh

# Runs on the diff, not the history. A secret already committed needs rotating,
# not a build that fails forever on an old commit.
secrets:
	./scripts/check-secrets.sh

# Needs the advisory database, so it is not in the offline set.
audit:
	cargo audit --deny warnings

sbom:
	cargo cyclonedx --format json --all

# ---------------------------------------------------------------------------
# Infrastructure
#
# `tf-validate` needs `terraform init`, which downloads the provider schema.
# The structural properties that a plan would *not* catch — the node pool
# having no public addresses, no workload identity holding delete on the
# evidence bucket — are asserted by Rust tests in `make test`, and those need
# no network at all.
# ---------------------------------------------------------------------------

infra: tf-fmt tf-validate

tf-fmt:
	terraform -chdir=$(TERRAFORM_DIR) fmt -check -recursive

tf-validate:
	terraform -chdir=$(TERRAFORM_DIR) init -backend=false
	terraform -chdir=$(TERRAFORM_DIR) validate

# Deliberately absent: an `apply` target.
#
# `terraform apply` against this configuration creates a GKE cluster, a KMS
# keyring and a set of service accounts in a real project, and a Makefile
# target is exactly the wrong affordance for that — too close to `make test`
# on the keyboard and in the mind. Applying is documented, with the
# impersonation flow it requires, in docs/security/credentials.md.

clean:
	cargo clean

help:
	@echo "check      the offline gates; this is the default and what CI runs first"
	@echo "all        check, plus release build, audit, sbom, terraform validate"
	@echo "fmt        rewrite formatting; fmt-check only reports"
	@echo "lint       clippy across the workspace, all targets, warnings are errors"
	@echo "test       the whole test suite"
	@echo "count      the suite's passing AND failing counts; red exits non-zero"
	@echo "e2e        the single end-to-end run through all seven layers"
	@echo "acceptance the workspace-level acceptance suite"
	@echo "build      release build, locked"
	@echo "deps       the dependency policy"
	@echo "secrets    secret scan over the diff"
	@echo "audit      cargo audit; needs the advisory database"
	@echo "sbom       cyclonedx bill of materials"
	@echo "infra      terraform fmt and validate; needs the provider schema"
	@echo "doc        rustdoc for the workspace"
