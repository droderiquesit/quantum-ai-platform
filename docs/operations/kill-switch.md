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
5. Clear the halt, which requires an operator credential authenticated within
   the last 15 minutes:
   ```sh
   curl -X DELETE -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" \
     .../api/v1/kill-switch
   ```
   A credential older than that is refused with `409`. Re-authenticate and
   repeat — the platform is asking you to prove you are still at the keyboard,
   not merely that you were an hour ago.

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

Every lift is recorded: who did it, how they authenticated, when, and the trip
they lifted. An incident review that can see what stopped the platform but not
who decided it was safe to continue is missing the more consequential half.
Clearing a halt that is not set is not an error and records nothing, because
nothing happened.

## What it does not do

It does not reduce the autonomy level, cancel working orders, or flatten
positions. Those are separate decisions, and a kill switch that made them
automatically would be one people are afraid to use.
