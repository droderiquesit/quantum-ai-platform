//! ADR 0100 §8: records the exogenous inputs a pass actually applied — tape
//! events, control frames and clock ticks — so replay can re-drive `run_pass`
//! from them verbatim. SLICE-24.
