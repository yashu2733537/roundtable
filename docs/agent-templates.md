# Agent templates

Built-ins live in `crates/roundtable-core/src/agent_templates.json` — six
today: Planner, Researcher, Coder, Reviewer, Writer, Summarizer. Users start
from one and edit the copy; templates themselves are read-only.

## Adding one

Add an object to the JSON array:

```json
{
  "key": "threat-modeler",
  "name": "Threat Modeler",
  "avatar_color": "#7c3aed",
  "avatar_icon": "◉",
  "role": "One line: what this agent is for (shown in lists).",
  "system_prompt": "The full job description. Be concrete: inputs, outputs, rules, and what to refuse.",
  "model": "",
  "temperature": 0.5,
  "max_tokens": 2000
}
```

Field notes:

- `key` — lowercase, unique, never shown; used for list order and tests.
- `model` — leave `""` so the user's default applies; set it only when the
  role genuinely needs a specific model family.
- `temperature` — 0.0–2.0. Planners/researchers 0.2–0.4, coders ~0.5,
  writers 0.7–0.9.
- `avatar_icon` — one or two characters; falls back to the name initial.

Then extend the `templates_parse_and_draft` test expectations in
`crates/roundtable-core/src/agents.rs` (count + spot-check the new entry)
and run `cargo test -p roundtable-core`.

## Writing a good system prompt

- State the role in one sentence, then the contract: what it receives,
  what it returns, and the shape of the return.
- Room agents: remind it of the tools it has (`send_message`, `delegate`,
  `finish`) — the runtime appends the full tool appendix automatically,
  so keep role-specific tool guidance to one or two lines.
- Forbid the failure mode you fear most (invented citations, placeholder
  code, unbounded plans) explicitly. Models follow prohibitions better
  than vibes.
