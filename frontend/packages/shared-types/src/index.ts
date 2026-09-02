/**
 * Types shared across every Algorik surface.
 *
 * This package once also carried a typed environment-configuration reader
 * (`readConfig`, `describeProblems` and the `AlgorikConfig` shape it
 * produced). A search of every surface found no import of any of them,
 * so the reader was removed rather than left to read as the place
 * configuration is validated when nothing routed configuration through it.
 */

/** Which deployment a surface is talking to. Renders as a colour everywhere. */
export type EnvironmentMode = "local" | "development" | "test" | "stage" | "production";

/**
 * The posture the surfaces display, which is not the same as the environment.
 *
 * `live` exists in the type because the platform can *report* it and an
 * operator must be able to see that it did. Nothing in any Algorik surface
 * selects it — see ADR 0014.
 */
export type TradingPosture = "simulation" | "paper" | "stage" | "live";
