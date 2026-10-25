# ADR 0210: A tool name is a letter or digit, then letters, digits and `._-/:`, and whitespace alone is no name

- **Status**: **Accepted**, 2026-10-24, decided by lane L6 under the owner's
  delegation of the same day ("Make all decisions for me",
  `docs/blueprint/owner-decisions-2026-10-24.md`). It amends one clause of
  ADR 0158 decision 17 and nothing else.
- **Amends**: ADR 0158 decision 17's character set.
- **Rows**: R12-SEC-9065.

## Context

ADR 0158 decision 17 made a tool's name a validated type, so that text read
from fetched content cannot become a tool invocation by being passed as a
`&str`. It gave the grammar as `[a-z0-9_.-]`. When `ToolName` was built
(`qip_agents::tools::ToolName::parse`), two names the platform already
registers fell outside it:

- the deep brain registers its hosted language model under the configured
  router id, which has the shape `organisation/model:provider` (its own
  test id is `example-org/example-model:example-provider`);
- connectors register as `connector/<source id>`.

Five deep-brain tests failed on the narrower set. Renaming either would
change a configured identifier operators already hold, for no gain in what
the type prevents.

## Decision

1. A `ToolName` is 1 to 128 bytes, starts with an ASCII letter or digit,
   and holds only ASCII letters, digits and `.`, `_`, `-`, `/`, `:`.
   Upper case is admitted because router ids carry it.
2. A name that is empty **or only whitespace** is refused as having no name
   ("a tool needs a name"), never as a grammar error, and an accepted name
   is never trimmed.
3. Everything else in ADR 0158 decision 17 stands: `ToolRegistry::propose`
   and `register` take a `ToolName`, and a `ToolName` is made only by
   `parse` (its `Deserialize` goes through it).

## What it costs

- `/` and `:` look like a path and a scheme. They are admitted as name
  characters only; nothing resolves a tool name as a path or a URL, and the
  grammar still refuses whitespace, quotes, `;`, parentheses and newlines,
  which are what turn a name into a command.
- A configured model id outside the set now stops qip-deepbrain's model
  assembly with a named refusal where it used to be accepted.

## What would make this wrong

- A caller that interprets a tool name as a path, URL or shell word. None
  exists; one that appears needs its own validation, not a wider name.
- A registered name that needs a character outside the set. The fix is a
  rename, recorded here, not a silent widening.

## Evidence

`backend/crates/libs/qip-agents/src/tools.rs` (`ToolName::parse`);
`backend/crates/libs/qip-agents/tests/expansion_registries.rs`;
`backend/crates/apps/qip-deepbrain/tests/research.rs::the_node_refuses_to_start_its_tool_registry_with_a_nameless_tool`;
qip-agents `tools::tests::a_tool_name_outside_the_grammar_is_refused_and_the_platforms_own_names_are_not`.
