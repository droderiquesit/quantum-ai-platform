---
name: incident-diagnosis
description: Diagnose a CI, pipeline, or runtime failure down to root cause. Use whenever something fails and the cause is not immediately obvious.
---

# Incident Diagnosis

**Trigger** — A failing workflow, a failing deployment, or a misbehaving process.

**Outcome** — A root cause with the evidence that establishes it, and a fix or a precise handoff.

## Prerequisites

Access to the logs or the failing command.

## Steps

1. Get the raw log to a file. Do not eyeball coloured terminal output — grep the saved file for the literal error string with a context window. ANSI escapes defeat naive matching and cost more time than the download.
2. Beware the echo: the first occurrence of a string is often the shell echoing the script; the second is the runtime output. Read both.
3. Find the first error, not the loudest. Later failures are usually consequences.
4. Reproduce it locally if you can. A failure you cannot reproduce is a hypothesis.
5. Establish root cause. 'Flake' is not a root cause — it requires evidence: a passing re-run on the same commit, or the same failure on the base branch.
6. Fix at the root. If the fix is outside your scope, hand off with the diagnosis and a proposed patch.

## On failure

If you cannot establish the cause, say what you ruled out and what evidence you would need. Never guess at a fix and push it.

## Result format

Root cause, the log lines that establish it, the fix, and confirmation the original failure no longer reproduces.

## Evidence

Real command output, quoted. Never report a step as done that you did not run.
A summary that omits a failure is a false statement about the system.
