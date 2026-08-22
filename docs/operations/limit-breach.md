# A risk limit is breached

## Do this

1. Identify the limit and by how much:
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/system/status
   ```
2. Understand what the platform has already done. The monitor escalates on its
   own:

   | Observation | Action |
   |---|---|
   | First breach | **Reduce-only.** New risk is blocked; reducing orders still pass. |
   | Third consecutive breach | **Scope halted.** |
   | Any breach at a live autonomy level | **Scope halted immediately.** |
   | Drawdown or daily loss past the threshold | **Global halt.** |

3. If the platform is reduce-only, it can still trade its way back inside the
   limit — that is what "reduce-only still permits reduction" is for. Watch
   whether it does.
4. If a scope is halted, the book is not coming back on its own. Decide whether
   to reduce manually or to accept the breach and raise the limit, which is a
   risk-committee decision rather than an operational one.

## Why the first breach does not halt

One reading can be a stale mark or a bad print. A platform that halts on a
single observation is a platform that any one bad tick can stop, and an
operator who has been woken by three false halts will start ignoring the
fourth.

Three consecutive observations, or four hours, is the threshold. At a live
autonomy level it is one, because the same breach is more serious when real
money is moving.

## Why reduce-only still permits reductions

A reduce-only state that blocked reducing orders could never fix the breach
that caused it. `MonitorAction::permits_reduction` is true for everything
except a global halt.

## Every limit has a rationale

```sh
qip limits
```

If a limit's rationale does not justify its current value, that is a finding
worth raising. `LimitSet::conservative_default` asserts every limit has a
non-empty rationale, so a limit nobody can explain does not ship.
