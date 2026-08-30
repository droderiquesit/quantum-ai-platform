# Policies

## Model governance

Every model the platform uses has a card in `qip_ai::registry` recording its
version, purpose, training data, evaluation history, limitations and owner.
`ModelRegistry::require_for_decision` refuses a model that is not in a
decision-eligible stage, and every investment decision references the models
that produced it.

A model whose drift exceeds its threshold, or whose last evaluation is stale,
becomes ineligible without anyone intervening.

## Agent governance

Every agent has a manifest declaring its purpose, competencies, limitations,
capabilities, budget, owner and review date. `Roster::validate` checks the
properties that only exist across the whole organisation:

* Somebody holds a veto, if anybody proposes trades.
* An adversarial function exists and does not report to the desks it reviews.
* Risk and trading have different owners.
* Escalation chains terminate.
* No two agents share an id.

An agent's authorisation expires after 90 days. A platform running on lapsed
authorisations is visible through `Platform::review_governance`, which an
operator is expected to run.

## Data licensing

`qip_financial::quality::LicensingClass` marks what each dataset may be used
for. `LicensingClass::Synthetic` is barred from production decisions outright.
The alternative-data agent takes its licensed-dataset list at construction and
will not use a dataset outside it — a licence question answered by an agent is
not answered.

## Change management

| Change | Requires |
|---|---|
| Code | a pull request, green CI, review |
| A risk limit | the above, plus a stated rationale (asserted non-empty) |
| An agent's capabilities | the above, plus `Roster::validate` still passing |
| The autonomy ceiling | the above, plus a Terraform apply |
| The autonomy level | two authenticated operators, at runtime, no code change |

The last row is deliberately the only one that is not a code change: enabling
live trading should be an operational decision with an audit trail, not a
deployment.
