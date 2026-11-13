# ADR 0429: An event-fabric topic moves schema version by additive change, in one commit across lock, binding, broker and consumers, and a prior version retires only on evidence

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-01, answering owner decision
  26 of 2026-10-31 ("the first move of a fabric topic from v1 to v2 needs one
  ADR on fabric versioning … covering the lock rows, the bindings, the broker
  registries and the versions each consumer admits; a single-row argument
  does not move a topic"). The delegation's limits hold: it sets no figure,
  changes no requirement's wording, adds no dependency and touches no
  paper-trading layer.
- **Date**: 2026-11-12
- **Refines**: [ADR 0100](0100-the-event-fabrics-first-build-is-an-in-tree-single-node-broker-and-a-separate-single-writer-ledger-on-std-tcp-with-producer-retained-durability.md)
  §5 (bodies are versioned) and the schema lock (CONTRACT-046/047,
  CICD-070, CICD-072). Changes none of their checks.
- **First applied by**: [ADR 0428](0428-a-market-state-pack-rides-in-the-passs-feature-digest-and-the-tick-desks-fusion-may-only-remove-a-weight.md),
  `reflex.feature_digested` v1 to v2.

## Context

Every check a version move needs already exists, and each sees only its
own piece:

- `qip-acceptance`'s `event_fabric_schema_lock` refuses a bound body whose
  shape moved without its schema id, and tells the author to "bump the
  schema version if the change is deliberate". It does not say what else a
  bump must move.
- `qip-events/schemas.lock.json` is embedded in `qip-events`, and
  `qip_events::registry::locked_descriptors` reads it. That is the set the
  broker admits by `(topic, schema_version)` (ADR 0156 §1.3, R12-FABRIC-8090).
- `qip-events/consumers.lock.json` is the consumer inventory that
  `qip_events::retirement` uses to refuse retiring a version somebody still
  reads (CICD-072).
- `event_fabric::bindings` sets the version a producer stamps on a record.
  Readers (`AnyEvent::decode`, `qip-edge-node`'s `writer::decode_as`) refuse
  a record newer than their binding and admit an older one.

Until now no topic had moved, so nobody had to say which of these move
together. Decision 26 says that question gets one answer, not one answer
per row.

## Decision

1. **A topic moves version exactly when its locked shape changes.** A shape
   change at an unchanged version stays refused by the lock, as now. Any
   change the lock sees counts as a move, however small.
2. **A version move is additive only.** The new version's type must decode
   every earlier version's body. In practice that means a new field that is
   an `Option`, or that carries `#[serde(default)]`. Removing or retyping a
   field is not a version move under this ADR. It needs its own ADR, and
   usually a new topic, because consumers could no longer read the old
   version's bodies.
3. **One commit moves all five pieces**, and the ADR that argues the move
   cites this one and states the evidence for each:
   a. **Binding**: the topic's `schema_version` in
      `event_fabric::bindings` rises by one, with a doc line naming the ADR.
   b. **Lock row and fixture**: the new row comes from the schema-lock
      suite's own regeneration output, never edited by hand. Its fixture
      carries the new field populated (`Some`, non-empty), so the lock
      records the nested shape.
   c. **Consumer survey**: `consumers.lock.json` gets a row at the new
      version naming every crate that decodes the type. A version with no
      survey is refused (CICD-072).
   d. **Broker registry**: nothing to edit by hand. The admitted set is
      `locked_descriptors()` over the embedded lock, so it moves with (b).
      The stream catalogues (`infrastructure/event-fabric/streams.*.json`)
      list topics, not versions, and stay unchanged. The move's ADR says so
      rather than assuming it.
   e. **Versions each consumer admits**: each consumer reads any version up
      to and including its binding and refuses a newer one. A test decodes
      an earlier-version body, through the consumer's real decode path,
      under the new type.
4. **A prior version may leave the lock in the same commit only on
   evidence.** All three must hold, and the move's ADR states each one:
   (i) every surveyed consumer decodes the new type, shown by 3e's test;
   (ii) no deployed process produces or stores the old version, shown by
   the deployment record for the producing binary (the committed tfvars)
   or by the retention of every store holding it having expired; and
   (iii) `qip event-fabric schema-gate` passes over the lock with the
   inventory, which the schema-lock suite runs. If any one fails, the old
   row and its survey stay until all three hold.
5. **A row argues a move; this ADR rules it.** A requirement row may need a
   new field. The move is lawful only when it meets clauses 1–4.

## What it costs

- A version move touches five files, two of them shared locks, so lanes
  working in parallel conflict there. That is deliberate: two lanes moving
  one topic at once is exactly the collision a conflict should surface.
- Only additive changes count as version moves, so a cleanup that removes a
  field costs a new ADR and usually a new topic.
- Keeping a prior version (clause 4 not met) leaves two rows for one topic.
  `event_fabric_schema_lock`'s gate test assumes the lock compared with
  itself reports "0 changed", and a two-version lock breaks that
  assumption. The first move that keeps a prior version must widen that
  assumption in the same commit. No move has done so yet.

## What would make this wrong

A deployed reader that refuses an older version, or a broker that admits by
a registry other than the embedded lock. Either one breaks clause 3d or 3e,
and this ADR must be revised before the next move.
