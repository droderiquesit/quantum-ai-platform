# The kill switch tripped

## Do this

1. **Do not clear it yet.** Nothing will be sent while it is tripped, which is
   the correct state until you know why.
2. Find the reason:
   ```sh
   qip status
   # or
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/system/status
   ```
   The **first** reason recorded is the trigger. Later ones are consequences —
   the switch keeps the first deliberately, because an incident review wants
   the cause rather than the last thing to notice.
3. Check whether open orders are still working. A halt stops *new* orders; it
   does not cancel working ones.
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/orders
   ```
   Cancel them if the halt reason means they should not complete.
4. Fix the cause.
5. Clear the halt. **`DELETE /api/v1/kill-switch` refuses every caller with
   `403` as of 2026-09-14 (ADR 0065), and no credential and no amount of
   re-authenticating changes that.** Restarting the `qip-api` process is the
   only way to lift a halt today: the switch is not resumed from the event
   log, so a fresh process starts unhalted.

   Read the next paragraph before you do it, because a restart is not a
   like-for-like substitute.

   **What the restart costs you.** The `KillSwitchClearance` record — who
   decided it was safe to continue, and when — is written only by the route.
   Restart and the log holds what stopped the platform and nothing about who
   restarted it, which is the more consequential half missing. Write the
   decision down somewhere durable yourself, with the trip it lifted and the
   cause you fixed at step 4, and say in the incident review that the record
   is absent because the route refuses rather than because nobody looked.

   **Why it refuses.** This step used to read: a credential older than fifteen
   minutes is refused with `409`; re-authenticate and repeat, because the
   platform is asking you to prove you are still at the keyboard. That
   sentence described a control that did not exist. `qip-api` reads
   `QIP_TOKEN_OPERATOR` once at start-up and stamped *that* instant on every
   credential, so the window measured the pod's uptime: a leaked token of any
   age passed for fifteen minutes after each restart, and every operator —
   including you, now — was refused for the rest of the process's life. A
   standing bearer token carries no authentication instant, so the platform
   refuses rather than offering one it does not have. Clearing a halt on a
   six-week-old token is the sharpest form of that attack, which is why this
   route was not exempted.

   Restoring the route needs a per-request proof of recency and a per-person
   subject, neither of which exists yet. Until then the asymmetry below is
   sharper than it was designed to be, and deliberately so.

## What tripped it

Any component can trip the switch and no component needs authority to. In
practice it is one of:

| Trigger | Meaning |
|---|---|
| `drawdown of N% reached the M% halt threshold` | The platform has been wrong for long enough that continuing on the same models is not defensible. |
| `a single-day loss of N%` | Something moved much faster than the models expected. |
| `N breach(es) unresolved after M observation(s)` | The book is not coming back inside a limit on its own. |
| `api:<subject>` | A human halted it. Ask them why before clearing. |

## Why clearing is harder than tripping

The asymmetry is deliberate. A false stop costs a few minutes of missed
opportunity; a missed stop costs whatever the platform does next. Stopping is
therefore free and restarting is not.

Clearing restores the *configured* level, not whatever was set last. The switch
overrides the level on read rather than mutating it, so a halt cannot silently
change what the platform does when it resumes.

Every lift **through the route** is recorded: who did it, how they
authenticated, when, and the trip they lifted. An incident review that can see
what stopped the platform but not who decided it was safe to continue is
missing the more consequential half. Clearing a halt that is not set is not an
error and records nothing, because nothing happened.

Since 2026-09-14 the route refuses and the only lift available is a process
restart, which records none of that — so on today's build this section
describes a guarantee the platform does not currently provide. It is kept
because it states what the record is *for*, and because the gap is the reason
step 5 asks you to write the decision down by hand. See ADR 0065.

## What it does not do

It does not reduce the autonomy level, cancel working orders, or flatten
positions. Those are separate decisions, and a kill switch that made them
automatically would be one people are afraid to use.
