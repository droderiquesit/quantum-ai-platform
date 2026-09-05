# Self-model and precedent routes

The read-only cognition surface of `qip-api`. Two `GET` routes under
`/api/v1`, both at the `viewer` role, both answering `200` with
`content-type: application/json`. `POST`, `PUT`, `PATCH` and `DELETE` on either
answer `405 {"error":"that method is not allowed here"}`. Nothing here grades,
re-grades, forgets or weights anything: the LEARN stage writes the self-model
and the REASON stage writes the precedents, and these routes read what those
stages wrote.

The Rust shapes are in `src/self_model_views.rs`. This file is the same
contract in prose, kept exact so a page can be built against it without
reading Rust.

## Conventions

- Keys are stable `snake_case`.
- Lists are ordered deterministically so two reads of the same state render
  identically: components by `(kind, key)` lexicographically, precedents in
  the order the kernel recorded them.
- A statistic the engine declined to compute is `null`, never a number. The
  minimum sample below which it declines is stated in the body.
- Every timestamp is RFC 3339 UTC.

## `GET /api/v1/cognition/self-model`

Every component the platform has graded — the detector whose class a thesis
carried, each roster analyst that contributed to it, and where charged, a
cost-router rung or a strategy family — with the size of its record and,
once the record reaches `minimum_sample`, the learning engine's estimated
accuracy.

```json
{
  "components": [
    { "kind": "analyst",  "key": "macro-analyst",     "samples": 1,  "accuracy": null,                  "calibrated": false },
    { "kind": "detector", "key": "price_dislocation", "samples": 10, "accuracy": "0.21428571428571427", "calibrated": true }
  ],
  "minimum_sample": 10
}
```

| Key | Type | Meaning |
|---|---|---|
| `components[]` | list | One row per component the engine holds a record for. Empty on a fresh platform; a component appears only once a thesis it produced has resolved informatively. Sorted by `kind` then `key`, both lexicographic. |
| `components[].kind` | `"detector"` \| `"analyst"` \| `"rung"` \| `"strategy"` | The engine's component kind in `snake_case`. |
| `components[].key` | string | The component's id within its kind: the hypothesis class for a detector, the manifest id for an analyst, the tier name for a rung, the family name for a strategy. |
| `components[].samples` | integer | Graded outcomes in the component's window. The window is bounded by the engine (128); older outcomes fall off. |
| `components[].accuracy` | decimal string or `null` | The engine's estimated accuracy — the hit rate shrunk toward one half by four pseudo-observations, `(hits + 2) / (samples + 4)` — as the exact text of the engine's own number. `null` whenever `samples < minimum_sample`: the engine refuses to estimate a thin record and the route does not fill the gap. Render `null` as "not yet measured", not as zero. |
| `components[].calibrated` | bool | Whether `accuracy` is present. Always equal to `accuracy != null` and to `samples >= minimum_sample`; the route refuses to serve a body in which those disagree. |
| `minimum_sample` | integer | The count below which the engine reports no accuracy. Present on every body, including an empty one, so a page can explain a `null` without a second request. |

## `GET /api/v1/cognition/precedents`

The precedent the REASON stage recorded beside each hypothesis: the resolved
episodes nearest to the situation when it asked, and how their outcomes sat
against the claim's direction. Each record is the kernel's own
`HypothesisPrecedent`, serialised as the kernel serialises it, in the order the
kernel holds them — oldest first, most recent last, bounded to the prediction
history.

```json
{
  "precedents": [
    {
      "hypothesis_id": "hyp-3f2a",
      "cycle": 2,
      "confidence": 0.64,
      "examined": 0,
      "memory_size": 1,
      "nearest": [],
      "digest": { "nearest": 0, "resolved": 0, "agreeing": 0, "agreement": null }
    }
  ]
}
```

| Key | Type | Meaning |
|---|---|---|
| `precedents[]` | list | One record per hypothesis REASON convened on. Empty until a cycle has reasoned. |
| `precedents[].hypothesis_id` | string | The hypothesis the precedent was recorded beside. |
| `precedents[].cycle` | integer | The cycle it was recorded in. |
| `precedents[].confidence` | number in `[0, 1]` | The hypothesis's effective confidence after review, copied so a reader sees it beside the digest. The digest did not move it. |
| `precedents[].examined` | integer | Candidates the memory's index examined before re-ranking. |
| `precedents[].memory_size` | integer | Episodes in memory when the question was asked. |
| `precedents[].nearest[]` | list | The recalled episodes, best first: `{episode_id, instrument, at, known_at, similarity, claim, decision, realised_move_bps?, agreed?}`. `realised_move_bps` and `agreed` are omitted where the episode has no outcome or no sign. |
| `precedents[].digest` | object | The `PrecedentDigest`, as the memory computes it. |
| `digest.nearest` | integer | Episodes recalled. |
| `digest.resolved` | integer | Of those, episodes with an outcome that has a sign. |
| `digest.agreeing` | integer | Of those, outcomes that went the claim's way. |
| `digest.agreement` | number in `[0, 1]` or `null` | `agreeing / resolved`, or `null` where nothing resolved has a sign — a share of nothing is not zero agreement, it is no evidence. |

## Errors

Same as the rest of the API: `401` with `www-authenticate: Bearer` for a
missing or unknown token, `403` below the viewer role, `429` over the rate
limit, `405` for a method other than `GET`, `503` if the platform lock is
poisoned. `/cognition/self-model` additionally answers
`500 {"error": ...}` if the `minimum_sample` it states is found to disagree
with the engine's behaviour on any row; that is a defect in this crate, not a
state a page should render.
