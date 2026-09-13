# Recalibrating a risk limit

The platform proposes; two people sign; a file is committed; a deployment
mounts it. Nothing in this runbook changes a bound in a running process,
because nothing can (ADR 0061).

## What you are looking at

The LEARN stage prices every order the gates refused and, per rule, keeps
score. When at least ten scored refusals charged to one rule have been
priced and three in four of them would have beaten standing aside, the
platform writes a **recalibration proposal**: the rule, its current bound,
the bound at which every regretted path would have been admitted, and the
evidence — how many paths, over what window, what they would have earned in
simulation, and every scored order id. It is a record on the event log
(`risk.rule_recalibration`) and it counts under
`qip_rule_recalibration_proposed_total{rule}`.

A proposal is withdrawn by the platform itself when the evidence stops
clearing the bar, so a signature cannot land on a finding that has since
evaporated. Beside the proposals you will also see **defences**
(`risk.rule_defended`: a rule whose declines were mostly correct, with the
loss it avoided) and **dormancy findings** (`risk.rule_dormant`: a rule
that has not fired for a hundred cycles across a hundred accepted orders).
Neither of those asks anything of you; they are context.

## Do this

1. Read what is proposed, and against which running set:
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/risk/recalibrations
   ```
   `limits` names the set this process booted on; `open` is what stands
   proposed; `history` is every proposal record — proposed, withdrawn,
   enacted.

2. Decide, as a risk-committee decision rather than an operational one,
   whether the evidence justifies the loosening. The proposal is the loosest
   defensible bound (it would have admitted every regretted path). Read the
   scored orders; check whether a second rule would still have refused them.

3. Sign, twice, as two different people, each within fifteen minutes of
   authenticating and the second within a day of the first:
   ```sh
   curl -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" \
        -H "Content-Type: application/json" \
        -d '{"rationale": "<why the desk accepts this bound>"}' \
        .../api/v1/risk/recalibrations/:rule/approvals
   ```
   The body takes `rationale` and nothing else. It cannot name the approver
   (that is your session) and it cannot name the bound (that is the
   platform's proposal). The first answer is `awaiting_countersignature`;
   the second, from a different subject, is `enacted` and carries
   `artefact`: the running limit set with exactly one bound replaced.

4. Commit the artefact. Save `artefact` verbatim under
   `data/risk-limits/<a name the desk will recognise>.json`, review it like
   any other change to configuration, and merge it.

5. Name it in the environment's tfvars:
   ```hcl
   risk_limits_file = "data/risk-limits/<that file>.json"
   ```
   The plan will show the file mounted on all three central roots — api,
   fastbrain and deepbrain — as `QIP_RISK_LIMITS_PATH`. It reaches all
   three because each assembles its own platform on its own set, and a bound
   moved on one brain and not another would be two desks with one name.

6. Redeploy. Each root reads the file once at boot, validates it against the
   shipped set, prints its name and SHA-256 in the banner (`risk limits:`),
   and runs under it. `sha256sum` on the committed file answers the same
   string. The operator page's risk panel and `GET /risk/recalibrations`
   both read the set the platform actually booted on.

## What the file may and may not do

It may move a bound. It may not remove a control: a file that does not
carry every limit the shipped set carries stops the process at start-up,
naming the missing limit. So do an empty set, a duplicated or unexplained
limit, a bound that is not a finite positive number, a warning threshold
outside `(0, 1]` and a critical multiple below one. A file that does not
read, or does not parse, stops the process too — a desk that believed a
loosening had been deployed and was silently running the shipped set would
find out from a refusal, at the moment the gap costs something to have
missed.

## Tightening

There is no route for it, on purpose: regret evidence can only ever argue
that a rule refused too much, and a body that could name a bound would be a
request to loosen a control rather than a signature on the evidence for one.
To tighten, edit the committed limits file (or start one from the shipped
set: `qip limits` prints it) and go through steps 4 to 6. That is a reviewed
commit, which is the review any change to a control gets.

## Why not apply it to the running process

Because then a request could move a control under a running book, and the
paper-trading boundary's neighbour would be one convenience away. The
acceptance suite refuses a `&mut self` method naming a limit on any type
that holds the set; the only way a bound reaches a process is the file it
reads at boot; and a redeploy is a diff somebody read.
