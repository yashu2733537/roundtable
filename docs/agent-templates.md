# Agent templates

Built-ins live as one markdown file per agent in
`crates/roundtable-core/agent_templates/` — 40 today, from Planner to Legal
Advisor. Users start from one and edit the copy; templates themselves are
read-only. A `build.rs` bundles them at compile time and rejects broken
frontmatter with the filename, so a bad template fails the build, not the
app.

## Adding one

Add one file, `crates/roundtable-core/agent_templates/<key>.md`:

```markdown
---
key: threat-modeler
name: Threat Modeler
avatar_color: #7c3aed
avatar_icon: ◉
role: One line: what this agent is for (shown in lists).
model: ""
temperature: 0.5
max_tokens: 2000
---
The full job description. Be concrete: inputs, outputs, rules, and what
to refuse. This body becomes the agent's system prompt verbatim.
```

Field notes:

- `key` — lowercase, unique, matches the filename; used for tests.
- `model` — leave `""` so the user's default applies; set it only when the
  role genuinely needs a specific model family.
- `temperature` — 0.0–2.0. Planners/researchers 0.2–0.4, coders ~0.5,
  writers 0.7–0.9.
- `avatar_icon` — one or two characters; falls back to the name initial.
- `avatar_color` — hex; the UI auto-picks readable text over it.

Then run `cargo test -p roundtable-core agents::` — the suite asserts the
count (bump it), unique keys, valid ranges, and that every draft passes
validation. No UI or shell changes needed: the template list flows through
the existing `agent_templates` command.

## Writing a good system prompt

- State the role in one sentence, then the contract: what it receives,
  what it returns, and the shape of the return.
- Room agents: remind it of the tools it has (`send_message`, `delegate`,
  `finish`) — the runtime appends the full tool appendix automatically,
  so keep role-specific tool guidance to one or two lines.
- Forbid the failure mode you fear most (invented citations, placeholder
  code, unbounded plans) explicitly. Models follow prohibitions better
  than vibes.
