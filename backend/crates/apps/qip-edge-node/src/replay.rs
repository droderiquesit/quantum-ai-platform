//! ADR 0100 §8: re-drives `run_pass` from recorded exogenous inputs — tape
//! events, control frames and clock ticks — against a `ManualClock`, never a
//! recording layer wrapped around the venue. SLICE-39.
