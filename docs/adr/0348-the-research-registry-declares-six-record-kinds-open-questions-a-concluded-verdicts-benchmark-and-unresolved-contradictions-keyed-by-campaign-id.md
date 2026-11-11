# ADR 0348: The research registry declares six record kinds: open questions are its Open entries, a concluded verdict cites its class's ADR 0192 benchmark, and an unresolved contradiction is a record keyed by campaign id that only a concluded campaign closes, with no variable-to-subject map (amends ADR 0152 decision 11)

- **Status**: **Accepted**, 2026-11-06. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation
  (`docs/blueprint/owner-decisions-2026-10-31.md`), on the ruling for
  R12-EXPAND-6061 (`docs/blueprint/lanes/wave1031-handoff-2-2026-10-31.json`,
  lane "L4b"), which its skeptic verified. The number is from L4b's block
  (decision 15). The delegation does not lift four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them. Every field it adds is a refusal or a
  record.
- **Date**: 2026-11-06
- **Amends**: ADR 0152 decision 11, which declined a benchmark suite as free
  text and declined an unresolved-contradictions list for want of
  ADR 0151's variable-to-subject map.
- **Keeps**: ADR 0151's refusal to invent that map; ADR 0192's benchmark
  table.

## Context

R12-EXPAND-6061 asks the research registry for six record kinds:
hypotheses, verdicts, experiments, open questions, benchmarks and
unresolved contradictions. `qip-agents`' `ResearchRegistry` holds the first
three. ADR 0152 decision 11 left the other three out. The benchmark was
refused as a free-text suite. The contradictions list was refused because
listing them by subject needs a map from detector variables to subjects
that ADR 0151 declines to invent.

Each of the three can be recorded without what was refused:
- An open question is already an entry: a hypothesis whose latest verdict
  is `Open`.
- A benchmark already exists per capability class. ADR 0192's table names
  each class's incumbent and gate, so a verdict can cite it as a typed
  reference rather than free text.
- A contradiction the detectors raise becomes a research item with a
  question, and the question has a campaign id (ADR 0152 decision 11's
  digest). Keyed by that id it needs no subject.

## Decision

1. **Open questions are the registry's `Open` entries.**
   `ResearchRegistry::open_questions` lists every hypothesis whose latest
   version is `Open`. No second list is kept.
2. **A concluded verdict cites a typed benchmark.** `BenchmarkRef` names the
   class, its incumbent and its criterion as ADR 0192's table gives them.
   `record_concluded` refuses a `Supported` or `Null` verdict without one.
   The expansion desk passes it on `conclude`: a fill-bias campaign's
   benchmark is ADR 0192's fill-bias row (the uncorrected twin, measured by
   `Criterion::IncrementalValue`), and a campaign whose evaluation does not
   carry that criterion has no benchmark the desk can name and is refused
   before anything closes.
3. **An unresolved contradiction is a record keyed by campaign id.**
   `record_contradiction(campaign id, evidence)` refuses empty evidence and
   an id that is not a campaign digest. Only `record_concluded` on the same
   campaign's question resolves it. `unresolved_contradictions` lists those
   still open. The desk records one when it admits an item raised by
   `GapTrigger::Contradiction`, citing the signal's evidence.
4. **Bounded.** The registry holds at most `MAX_CONTRADICTIONS`. Past it the
   oldest resolved record is given up first, and a registry full of
   unresolved ones refuses the next rather than dropping an open one.
5. **No variable-to-subject map.** Nothing here names a subject. ADR 0151's
   refusal stands.

## Consequences

- A concluded verdict can be read back with the benchmark it was judged
  against, and a contradiction can be read back as open until a campaign on
  its question concludes.
- A new capability class that the desk concludes needs its ADR 0192 row
  named in the desk before its verdicts can be recorded.

## What it costs

- One more refusal on `conclude`: a campaign with no nameable benchmark no
  longer records a verdict. Today only fill-bias campaigns reach it.
- A contradiction raised again on an already-open campaign merges its
  evidence into the open record rather than opening a second one.

## What would make this wrong

- A detector contradiction that no campaign could ever conclude (one the
  desk refuses at admission). Then its record never resolves, and the cap
  is what bounds it; an expiry would be needed.
- A second capability class reaching `conclude` with a benchmark ADR 0192
  does not name. Then the table needs its row before this desk records it.
