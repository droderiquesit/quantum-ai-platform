# R12-EVID-2031: ADR text for owner (new ADR; number must be claimed in docs/adr index first)

Title: An unclaimed record is not admitted by default

Status: proposed, from owner decision 36 (2026-11-14, fail closed).

Decision: only the named set of arms whose records are venue or vendor
market-data bars (qip-fastbrain `Feed`: Synthetic, Replay, Tape, Live) is an
observation, not an extracted claim, and may reach `Platform::observe` with
claim `None`. Every other arm (the connector arm, and any new arm) must mint an
`EvidenceRecord` and carry its claim id. `Feed::is_observation_arm` is the
exhaustive, wildcard-free statement of the set; `Feed::poll` refuses a `None`
claim from an arm outside it.

Consequences: a new arm does not compile until classified; the default is to
mint evidence. The fact-to-evidence link (EvidenceIntake::release drops the
claim id) stays open.
