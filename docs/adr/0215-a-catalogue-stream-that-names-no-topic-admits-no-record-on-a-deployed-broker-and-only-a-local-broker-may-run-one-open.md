# ADR 0215: A catalogue stream that names no topic admits no record on a deployed broker, and only a local broker may run one open

- **Status**: **Accepted 2026-10-31 under the owner's delegation** ("complete
  all sessions ... finish what you can by morning", 2026-10-31, extending
  `docs/blueprint/owner-decisions-2026-10-25.md` and
  `docs/blueprint/owner-decisions-2026-10-24.md`). The limits of that
  delegation hold here: no risk number, no limit, threshold or limit set
  changes, no billed infrastructure, and no paper-trading layer is touched.
  Conservative and fail-closed by construction: a deployed broker admits
  less after this, never more.
- **Date**: 2026-10-31
- **Decides**: the half of ADR 0156 decision 4 that could not land — "a
  stream that names no topics is refused" — against the committed
  catalogue, whose `telemetry.local` names none.
- **Extends**: ADR 0156 (schema admission by `(topic, version)`), ADR 0100
  §5 (the P4 telemetry row), ADR 0110 decision 5 and ADR 0112 (what a
  `local` deployment may do that a deployed one may not).
- **Rows**: R12-FABRIC-8090.

## Context

ADR 0156 decision 4 said a catalogue stream that names no topics is refused
instead of "unrestricted, as before", because a stream that admits
everything is the hole the topic gate was written to remove. Its own
consequences section said the committed catalogue must be read first, and
it was: `infrastructure/event-fabric/streams.local.json` and
`streams.managed.json` both declare `telemetry.local` (class P4) with
`"topics": []`, because no body in `schemas.lock.json` can ride a P4 stream
and `qip-cli`'s `produce` chooses no synthetic topic for P4 by design. So
the refusal was left out (`Service::apply_catalogue` registers no admitted
set for a topic-less stream and the broker admits any record on it), and
R12-FABRIC-8090 stayed PARTIAL on that one clause for twelve days.

Two facts bound the decision. Nothing produces to `telemetry.local`: no
shipped binary holds a produce grant on it, and the acceptance suite says
so (`every_process_identity_in_the_slice_holds_exactly_the_grants_the_slice_uses`:
"Nothing produces to telemetry.local in this slice"). And every in-process
fixture — `qip-fabricd`'s own suites, the ledger's, the edge node's, the
CLI's — declares its streams with `"topics": []` and produces raw payloads
that are not `AnyEvent`s, so a refusal applied to every broker would stop
every one of those suites for a property none of them is about.

## Decision

1. **A deployed broker admits no record on a stream that names no topic.**
   `Service::apply_catalogue` takes a `TopiclessStreams` policy. Under
   `TopiclessStreams::AdmitNone` a stream with an empty topic list is
   registered with an empty admitted set, so `Broker::require_admitted_topics`
   refuses every record on it with `SchemaRefused`, naming the stream and
   the fact that it admits no topic, before anything is appended. The
   stream still exists: its metadata answers, its partitions are declared,
   an operator may isolate and release it. It carries nothing until the
   catalogue names what it carries.
2. **The policy is the deployment's, decided in the composition root.**
   `qip-fabricd` maps `Deployment::Local` to `TopiclessStreams::AdmitAll` and
   every other deployment to `AdmitNone`, on the first catalogue load and on
   every reload. `Local` is the only deployment under which a reproducible
   key or a plaintext link is admitted (ADR 0110 decision 5, ADR 0112); an
   open stream joins that list, for the same reason and with the same
   boundary. The policy is a parameter of `Service::new`, so a service
   cannot be built without saying which it is.
3. **`telemetry.local` stays declared and names nothing.** It is removed
   from neither committed catalogue, so a deployed broker refuses every
   record on it rather than a producer discovering at start that its stream
   is gone. When a P4 body is locked (a row in `schemas.lock.json` and a
   topic the catalogue can name), the stream names it and this decision
   stops mattering for that stream.
4. **Not decided here**: what a P4 telemetry body is. Inventing a record
   type nobody produces so the stream could name it would be a registered
   constant nothing calls, which is the shape of control this platform's
   rules refuse.

## Consequences

- On a deployed broker a topic-less stream is closed, where it was open.
  Nothing shipped produces to the one such stream, so no deployed path
  changes behaviour, and a future producer pointed at it is refused with a
  message that says why.
- Every in-process fixture keeps running unchanged under `AdmitAll`, which
  it declares.
- A `Service` has one more required argument. Seven call sites name it.
- The test `a_deployed_broker_admits_nothing_on_a_stream_that_names_no_topic`
  in `qip-streaming` proves decision 1 and `a_deployed_configuration_closes_topic_less_streams_and_a_local_one_does_not`
  in `qip-fabricd` proves decision 2; both are mutation-verified in the
  commit that lands them.

## What it costs

- One more required argument on `Service::new`, named at every call site.
- A deployed broker's P4 class has no admissible traffic until a P4 body is
  locked, so "sample or shed" has nothing to sample on a deployment. That
  was already true in fact — nothing produced to it — and is now true by
  refusal, which is the honest state.
- The guard's one wiring line in `qip-fabricd` (`topicless_streams(config.deployment)`
  passed to `Service::new`) is proven by the mapping's test and the
  service's test on either side of it, not by a deployed fabricd in a test,
  because no suite can start a non-`local` broker without its CA Service
  certificate. Stated rather than hidden.
- Nothing billed, nothing deployed, no risk number.

## What would make this wrong

- A P4 producer that ships before a P4 body is locked: it would be refused
  on every deployment but `local`, which is the point, and the fix is the
  lock row, not a wider policy.
- A deployment that needs an open stream for a reason other than a
  fixture. None is known; if one appears it needs its own record, because
  "open" is the hole ADR 0156 named.
