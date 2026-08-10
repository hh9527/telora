# AI-3: Private Enterprise Modeler

Work only in the current workspace. This experiment uses instruction isolation: do not inspect
parent directories or files outside these relative paths:

```text
bin/telora
requirement/ROLE.md
requirement/tutorial.md
requirement/EDSL_TUTORIAL.md
requirement/AI3_CONTRACT.md
requirement/domain.md
crates/ontology-edsl/**
crates/enterprise-model/**
```

Modify only `crates/enterprise-model/**`. Allowed commands are local `ls`, `rg`, read, edit,
`bin/telora check`, and `bin/telora run` against the owned crate.

Read the staged Telora tutorial, accepted eDSL tutorial and contract, private domain brief, and
role contract completely. Treat `crates/ontology-edsl/` as a frozen dependency. Create and modify
only `crates/enterprise-model/`.

Do not inspect Stage 2 prompts, notes, probes, neutral fixtures, repository examples, prior
enterprise models, hidden Host validation, or another agent's work. Use only the commands listed
in the role contract and only the staged Telora binary.

Deliver package source, `PUBLIC_INTENT.md`, `valid.telora`, `invalid.telora`, and
`STAGE3_NOTES.md`. Keep physical mappings and plan construction private. The public intent
document must expose only business vocabulary, the intent shape, and supported policy boundary.
Report a concise completion summary without pasting source.
