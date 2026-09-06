/**
 * Whether this console requires a signed-in session before it forwards
 * anything to the platform.
 *
 * **On unless `ALGORIK_AUTH_REQUIRED=false`.** This used to be the other way
 * round — off unless the variable was `"true"` — and the failure that inverts
 * it is the quiet one: a deployment that forgot the variable, or misspelled
 * it, or lost it in a template, served the whole console anonymously and
 * attached the platform's own bearer token to every anonymous request. Every
 * smoke test passed, because a console that answers is indistinguishable from
 * one that answers only the signed-in.
 *
 * The open form still exists — a kiosk on a desk, and the Playwright suites
 * that predate accounts — but it is now something a deployment has to write
 * down, in the one spelling this function accepts. Any other value, including
 * absence, is the closed gate. `.claude/rules/01-security-and-safety.md`:
 * every safety default is the restrictive one.
 *
 * Read per request rather than at module load, like the upstream target, so
 * one build serves both postures and a test can prove each.
 */
export const AUTH_REQUIRED_VARIABLE = "ALGORIK_AUTH_REQUIRED";

export function authRequired(): boolean {
  return process.env[AUTH_REQUIRED_VARIABLE] !== "false";
}
