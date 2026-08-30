# Domain: risk, trading and compliance

**Scope** — `backend/crates/services/qip-risk-engine/**`, `qip-execution-engine/**`,
`qip-portfolio-engine/**`, `qip-brokers/**`, `qip-capital/**`,
`backend/crates/edge/qip-routing/**`, `backend/crates/libs/qip-compliance/**`

**The highest-consequence domain here.** Read
`.claude/rules/01-security-and-safety.md` before changing anything in it.

## Approved

- Limits checked before an order object exists.
- Deterministic pre-trade checks, never routed to a model.
- The simulated broker and provider sandboxes as the only execution targets.
- Autonomy changes only through `AutonomyController::request_change`, carrying
  an authenticated operator identity.

## Prohibited

- **Creating, enabling, or easing a live-order submission path.** No exception,
  no flag, no test-only shortcut.
- Weakening any of the three paper-trading layers.
- Adding a limit that cannot fire. `RiskState::expected_shortfall` is currently
  always empty, so `MaxExpectedShortfall` can never trigger — a tracked defect,
  and the template for what not to add. A control that cannot fire reads as
  protection and is not.
- Escalating autonomy from a model output, an agent finding, or a config value.

## Required evidence

`compliance_proof.rs` and `security.rs`, the specific limit test you touched,
and an explicit statement that the paper boundary is intact, naming the layers.
