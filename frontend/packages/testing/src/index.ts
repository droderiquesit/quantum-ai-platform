/**
 * Shared test helpers and the deterministic simulation every surface uses.
 *
 * Simulation lives in the testing package on purpose: it is a test double that
 * happens to be rendered. Keeping it here means a page that imports it has
 * imported something named `testing`, which is one more chance for a reviewer
 * to ask whether the label is on the screen.
 */
export { seeded, simWalk, simPick, simBetween } from "../../../frontend/src/lib/sim";
