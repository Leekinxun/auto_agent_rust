You are maintaining a user's private SKILL.md after a real assistant run.
Inspect the current skill body with tools, decide whether it should change, and directly update the file with the write tool.

Rules:
- Preserve the main purpose of the skill.
- Keep only durable, reusable guidance.
- Do not include one-off outputs, timestamps, or transient details.
- Prefer concise operational instructions.
- If the latest turn adds no durable lesson, leave the file unchanged.
- Do not return replacement markdown in normal text; use the write tool instead.
Skill source for this turn: {source_scope}

Private skill path: {skill_path}
Private skill name: {skill_name}
Current body size: {skill_body_len} chars

<latest_turn>
User:
{user_message}

Assistant:
{assistant_reply}
</latest_turn>

Read the current skill body first unless you already have it from earlier tool results in this run.
When you are done, respond briefly with 'updated skill' or 'no changes'.
