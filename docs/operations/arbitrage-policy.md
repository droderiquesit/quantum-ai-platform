# The arbitrage desk's policy

## Do this

1. To run with **no desk**, leave `QIP_ARBITRAGE_POLICY_PATH` unset. That is
   the deployed default. Every cycle whitelist ships empty, every cell's
   installer declines with `EmptyWhitelist`, and the cycle response says so:
   ```sh
   curl -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" .../api/v1/cycle
   # "mesh": { "policy": { "whitelist": [
   #   "cycle whitelist for london-1: empty, CentralConfig::arbitrage is unset" ] } }
   ```
2. To permit a desk, write an `ArbitragePolicy` as JSON (fields below), mount
   it as a file, and point `QIP_ARBITRAGE_POLICY_PATH` at it — the way the
   instrument universe is mounted at `/etc/qip/universe.json` and named by
   `QIP_UNIVERSE_PATH`. Restart `qip-api`; the banner's `arbitrage desk:` line
   states what was read.
3. Confirm the desk's strategy holds a **live grant** at the cell. The
   whitelist is sized from that grant and nothing else; without it the
   whitelist ships empty and the cycle says
   `<strategy> holds no live grant there`.
4. Read the whitelist line on each `POST /cycle`. It is also written to
   stderr, prefixed `qip-api:`, and journaled on `PolicyDistributed`.

## What the policy says

The centre holds no pair list and no fee schedule. Portfolio optimisation
solves single-instrument problems, the venue fee schedule lives at the edge,
and the only venue fact the centre owns is the venue list on each grant it
issued. So what the desk may price is stated by an operator, held as data on
`CentralConfig::arbitrage`, and joined to the one fact the centre does own:
the desk strategy's live capital grant at each cell, whose per-order limit is
the start size in the funding instrument.

```json
{
  "strategy": "arb-desk",
  "funding_instrument": "USD",
  "venues": {
    "XNYS": { "class": "Exchange", "taker_cost": "0.0005" }
  },
  "markets": [
    { "venue": "XNYS", "market": "AAA-USD@XNYS", "base": "AAA", "quote": "USD" }
  ],
  "start_sizes": { "AAA": "100" }
}
```

| Field | Meaning |
|---|---|
| `strategy` | The desk's strategy id — the name the edge node's installer is configured with. The grant is looked up by it; a grant for any other strategy funds no desk. |
| `funding_instrument` | The instrument the grant is denominated in. Its start size is the grant's per-order limit, never a value here. |
| `venues` | Every venue a market may name, keyed by venue id. `class` is one of `Exchange`, `DarkPool`, `Ecn`, `Derivatives`, `OverTheCounter`, `CryptoExchange`; `taker_cost` is a proportional cost in `[0, 1)`, written as a decimal string. |
| `markets` | The books the desk may price, in the order its conversions will be listed. Each names its venue, the book's own instrument id, the `base` it is priced in units of and the `quote` it is priced against. Each book becomes two trade edges: buying the base out of the quote on the ask, selling it back on the bid. At most 128 markets. |
| `start_sizes` | How much of each non-funding instrument a cycle may commit, in that instrument's units, as decimal strings. The centre holds no price to convert a grant into these, so they are stated. |

Unknown fields are refused. Money and costs are decimal strings, never
floating point.

Whether any cycle is taken is the cell's decision, made alone against its own
books (ADR 0008). The policy names what may be priced; it is never an order,
and it cannot make a paper-trading cell submit one.

## What refuses, and where

**At start-up.** `QIP_ARBITRAGE_POLICY_PATH` set to a file that cannot be
read, or to one that is not a policy, stops `qip-api` naming the path. A
process that fell back to "no desk" because the named file was missing would
run healthy with the desk silently off. Then the plane assembles the policy
and refuses, naming the entry, any of:

* no strategy, no funding instrument, or no market;
* more than 128 markets (a cell walks at most 256 conversions per pass);
* a venue with an empty id, or a `taker_cost` outside `[0, 1)`;
* a market at a venue `venues` does not describe, or with an empty market,
  base or quote, or whose base equals its quote, or listed twice;
* a venue no market names;
* a funding instrument no market trades, or a start size for the funding
  instrument itself (the grant sizes it);
* a start size that is not positive, for an instrument no market trades, or
  a traded instrument with no start size.

**At each cycle**, per cell. These do not stop the process; they ship the
slot **unproduced** and say why in the cycle's `whitelist` line, prefixed
`not shipped`:

* the policy trades at a venue the desk's grant at that cell does not permit
  (the line names the venue);
* the grant permits no order (its order limit is not positive).

An unproduced slot reads as unavailable at the cell, which narrows the cell's
sizing under its freshness table. The cell never receives a whitelist the
centre had to guess at.

**Empty, not refused.** An unset policy, or a policy whose strategy holds no
live grant at the cell, ships a *produced, empty* whitelist and is journaled
as such. The installer reads it as `EmptyWhitelist` and installs nothing.

## Reading what happened

* `POST /api/v1/cycle` — `mesh.policy.whitelist`, one line per cell.
* The event log, topic `PolicyDistributed` — one `WhitelistIssue` per cell
  per cycle, empty ones included, with the outcome and the grant signature it
  was sized against. A refusal is not journaled: nothing was distributed.
* The cell's delta stream — its installer's own verdict, which agrees with
  the centre's line because the centre refuses everything the cell would.

The slot's time-to-live at the cell is sixty seconds; a desk is built only
from a fresh whitelist, so a centre that stops cycling withdraws every desk
within a minute.
