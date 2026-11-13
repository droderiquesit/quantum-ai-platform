# ADR 0410: A registry is versioned when a successor takes the next version, a stale or repeated one is refused and moves nothing, and no version is rebound to other content

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session cov-a-01
  writes it. The limits of that delegation hold:
  - it sets no figure, limit or threshold, and decision 2 below exists
    because none was given;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge.
- **Date**: 2026-11-12
- **Extends**: ADR 0198 (`promote` is the one door into production for a
  fitted model). It amends no clause of any accepted ADR.
- **Rows**: R12-COVERAGE-7170.

## Context

R12-COVERAGE-7170 asks that the source, ontology, memory, brain, tool,
model, research and capability registries be "versioned and extensible
rather than hard-coded". The ontology, research and source registries
already refused a version not above the latest per name. Four had no such
rule: a tool's scope was widened in place; a brain's `version` field had no
door that could move it, because `BrainRegistry::register` refused a second
spec and named no successor path; a memory entry re-admitted under a held id
sat beside the first with nothing to say which was current; and a model card
re-registered under a promoted `name@version` swapped the artifact that
version named. No one place said what "versioned" means. An earlier
session's draft (`docs/blueprint/lanes/l6-c1-01-coverage-7170-wip.patch`)
built most of the four and kept superseded brain and source versions to a
history of sixteen each, a figure nobody gave.

## Decision

1. **The rule.** A registry is versioned when a first registration is
   admitted, a successor is admitted as the next version, and a stale,
   equal or duplicate version is refused and moves nothing.
   `qip-acceptance`'s `registry_versioning` suite drives every one of the
   eight registries through its public door against that rule, each test
   asserting its premise first.
2. **No figure bounds a history, so a registry whose history could grow with
   uptime keeps the version in force and its lineage.** The brain registry
   admits a successor through `BrainRegistry::succeed`, only as exactly the
   held version plus one and only when its `parent_version_id` names the
   held version (`brains::version_id`); the superseded spec is named by that
   parent and not retained. The source registry is unchanged: the version in
   force rises and an older re-assessment is refused. The draft's
   sixteen-entry histories are not built. A history worth reading is the
   event log's, and a bound on a working copy of it is the owner's figure.
3. **The model registry's version is unordered, and the rule is met by
   binding.** `ModelCard::version` is a string; ordering strings would
   invent a convention. `name@version` names one artifact: once a held card
   is bound to an artifact digest, `ModelRegistry::register` refuses a card
   under the same reference carrying another digest or none. A successor is
   a new reference beside the old one. A card revising what is recorded about
   the same artifact (its drift score, its evaluations) is still admitted.
4. **The tool registry mints its own numbers.** `ToolRegistry::register`
   records version one; each `promote` that widens the scope records the
   next, citing its evaluation; a grant already held mints nothing. No door
   takes a version from a caller, so a stale one cannot be offered. The
   history is bounded without a figure: a scope only widens, so a tool holds
   at most one version per `ToolPermission`.
5. **Memory is versioned within its working set.** `MemoryRegistry::admit`
   numbers an entry as the next version of its id; `admit_version` takes a
   producer's number and refuses zero and a number not above the latest
   held, counting the refusal. The latest is read off the held entries, so
   an id whose every version has left the bounded working set
   (`RETAINED_MEMORY_ENTRIES`) is unknown again and starts at one. The event
   log is the record.

## What it costs

- A brain's or a source's superseded version cannot be read back from its
  registry. A decision made under it is checked against the event log, or
  not at all where nothing journals the registry.
- A model whose bytes change must take a new version string; a desk that
  reused version strings must stop.
- A memory id that ages out of the working set loses its numbering, so two
  version ones of one id can exist across the life of the node, at
  different times.

## What would make this wrong

- An owner's figure for a history bound: the brain and source registries
  then keep that many predecessors, and decision 2 is superseded.
- A consumer that must read a past tool, brain or source version at decision
  time from the registry rather than the log: that consumer names the bound
  it needs.
