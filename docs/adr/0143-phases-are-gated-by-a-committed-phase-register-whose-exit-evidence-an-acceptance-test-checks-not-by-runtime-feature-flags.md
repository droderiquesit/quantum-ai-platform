# ADR 0143: Phases are gated by a committed phase register whose exit evidence an acceptance test checks, not by runtime feature flags

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It adds a document and a test; it
  removes no capability, adds no flag and changes no deployment.
- **Date**: 2026-10-18
- **Extends**: the register's practice of naming the exact test that proves a
  row (`verification` on every assessment row).
- **Keeps**: fail-closed defaults where a switch already exists (for example
  `execution_nodes_enabled`, `workload_metrics_exist`), ADR 0003.

## Context

R12-ARCH-7134 asks that "a phase's exit capability is demonstrated before
the next phase's capabilities are enabled". The tree carries every phase at
once: agency, ambient intake and quantum research exist beside a fabric that
has not reached its multi-region phase, and nothing records which phase a
capability belongs to or whether its predecessor's exit was shown. The row's
own evidence is two acceptance suites (`m5_critical_path.rs`,
`m6_foundation.rs`) and the finding "no phase gate artefact in code".

Two readings are available. A runtime gate — every later-phase capability
behind a flag that stays off until an exit check passes — would mean
threading flags through most crates and switching off code that runs and is
tested today, for an ordering the deployments already enforce by what they
deploy. A record gate — a committed register of phases, each with its exit
capability and the tests that demonstrate it, checked on every push — gives
the row's property (exit demonstrated before the next phase is entered)
without disabling anything.

## Decision

1. **A phase register is committed** at `docs/architecture/phase-register.json`:
   each phase has an id, the blueprint section it comes from, its exit
   capability in one sentence, the acceptance tests that demonstrate it (by
   crate, file and function name), its state (`entered`, `exited`), and for
   each capability present ahead of its phase, the reason and the switch (if
   any) that keeps it off in deployment.
2. **An acceptance test reads the register** and refuses: a phase marked
   `entered` whose predecessor is not `exited`; a phase marked `exited` whose
   named tests do not exist in the tree (matched as a delimited function
   name, not a substring); and a register whose phases do not form one
   ordered sequence. Because CI runs the acceptance suite on every push, a
   phase's exit evidence is re-demonstrated continuously, not once.
3. **No runtime feature gating is introduced.** A capability present ahead of
   its phase is recorded as such. Where a deployment switch already keeps it
   off, the register names the switch; where none does, the register says so
   plainly rather than inventing one.
4. **Moving a phase's state is a reviewed commit** of the register, carrying
   the test run that shows the exit tests passing; the register is not
   updated by any automated process.

## Consequences

- "Which phase are we in, and was the last one's exit shown?" has one answer
  a person can read and a test re-checks.
- Capabilities present ahead of their phase become visible as a list, which
  is uncomfortable and is the point.
- Code slices left: the register populated from the blueprint's phases and
  the existing suites, and the acceptance test with its mutations (mark a
  phase entered over an unexited predecessor; rename a named test)
  (ARCH-7134).

## What it costs

- A document that must track the blueprint's phases and the test names;
  renaming a test now means editing the register in the same commit or the
  suite fails.
- The row's literal "before ... enabled" is met by recording and re-checking,
  not by switching code off; a reader who wanted code disabled will find this
  record weaker than that, and says so here.

## What would make this wrong

- If a later-phase capability present ahead of its phase ever caused harm in
  a deployment because nothing kept it off, decision 3 would have been the
  wrong trade for that capability, and it needs a real switch.
- If the register's named tests passed while the exit capability did not hold
  (tests that assert too little), the gate would be ceremony; each named test
  must be one the register's reviewers have mutation-verified.

## Rows

R12-ARCH-7134. It stays PARTIAL until the register and its test land with
their mutations.
