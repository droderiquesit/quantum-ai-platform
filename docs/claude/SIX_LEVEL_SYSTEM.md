# The six-level system

One map of how the Claude configuration in this repository fits together.
Higher levels set constraints and intent; lower levels execute within them and
return evidence upward.

| Level | Purpose | Where it lives |
|---|---|---|
| 1 | Governance and safety | `.claude/settings.json`, `.claude/rules/00–02`, `.claude/hooks/`, `docs/claude/managed-policy-recommendations.md` |
| 2 | Product vision and direction | `CLAUDE.md`, `.claude/rules/10-product-direction.md`, `docs/product/` |
| 3 | Architecture and domain intelligence | `.claude/rules/architecture/`, `.claude/rules/domains/`, `docs/architecture/`, `docs/adr/`, nested `CLAUDE.md` |
| 4 | Specialist agents | `.claude/agents/` (12 specialists) |
| 5 | Delivery workflows and gates | `.claude/skills/` (8 skills), hooks, permissions |
| 6 | Orchestration and verification | `.claude/agents/chief-orchestrator.md`, `docs/claude/AUTONOMOUS_DELIVERY_WORKFLOW.md` |

## How the levels actually load

Being precise about the mechanism, because "there is a rules directory" is not
the same as "the rules are loaded":

- **`CLAUDE.md` is loaded automatically** in every session, and it `@`-imports
  the four always-on rules — governance, security, change management, product
  direction. Those four are therefore always in context, and they are kept
  short for exactly that reason.
- **Domain and architecture rules are referenced, not auto-loaded.** They are
  read when work touches their scope. This is deliberate: loading six domain
  rules into every session would cost context in every session to serve one.
- **Nested `CLAUDE.md` files** in `frontend/` and `infrastructure/` load when
  work happens in those trees. They exist only because the *commands* there
  differ — `npm` and `terraform`, not `cargo`. A nested file that merely
  repeated the root would be a maintenance liability.
- **Agents and skills are discovered** from `.claude/agents/` and
  `.claude/skills/`.
- **Hooks and permissions are enforced by the harness**, not by a model
  agreeing to follow them. That distinction is the whole reason Level 1 uses
  them rather than prose.

## Enforcement versus instruction

| Control | Enforced by | Can a model bypass it? |
|---|---|---|
| Secret file reads | `permissions.deny` | No |
| Dangerous commands | PreToolUse hook | No — and it returns the safe alternative |
| Rust formatting | PostToolUse hook | Not applicable; it just runs |
| Push, cloud, apply | `permissions.ask` | Only with the user's approval |
| Everything else | Prose in rules | Yes, which is why safety-critical items are above this line |

Anything that must hold belongs in the top four rows. Prose is for judgement,
not for boundaries.

## A known name collision

`.claude/skills/security-review/` shares a name with a built-in skill of the
same name. The path was specified, so it is kept; if invocation ever resolves
to the wrong one, invoke the agent `security-engineer` instead, which is
unambiguous.

## What this configuration does not do

- It does not make managed policy. A repository cannot govern its own
  organisation, and a control an agent can edit is one an agent can remove.
  See `docs/claude/managed-policy-recommendations.md`.
- It does not replace review. Independent human judgement is the last gate.
- It does not authorise deployment. Production remains a human decision.
