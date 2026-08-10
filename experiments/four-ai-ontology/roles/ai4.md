# AI-4: Intent Author

Work only in the current trial workspace. This experiment uses instruction isolation: do not
inspect parent directories or files outside these relative paths:

```text
requirement/ROLE.md
requirement/INTENT_TUTORIAL.md
requirement/PUBLIC_INTENT.md
requirement/PUBLIC_API.md
requirement/REQUEST.md
crates/intent/**
```

Modify only `crates/intent/intent.telora` and `crates/intent/NOTES.md`. Allowed commands are local
`ls`, `rg`, read, and edit. Do not run programs.

Read every staged requirement file. Translate the single request into public intent Telora using
only the published vocabulary and interface. Create or modify only `crates/intent/intent.telora`
and `crates/intent/NOTES.md`.

Do not inspect the enterprise implementation, eDSL implementation, private domain, physical
mappings, hidden acceptance classification, other trials, or another agent's work. Do not emit
SQL, a physical plan, or a hand-built execution plan. If the closed public vocabulary cannot
faithfully express the request, document a refusal and do not invent an identifier.

Use only the commands listed in the role contract. Report a concise completion summary without
pasting source.
