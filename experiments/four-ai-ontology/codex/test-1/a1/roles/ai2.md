# AI-2: Ontology eDSL Implementer

Work only in the current workspace. This experiment uses instruction isolation: do not inspect
parent directories or files outside these relative paths:

```text
bin/telora
requirement/ROLE.md
requirement/tutorial.md
requirement/edsl-design.md
requirement/FEEDBACK.md (correction rounds only)
crates/ontology-edsl/**
```

Modify only `crates/ontology-edsl/**`. Allowed commands are local `ls`, `rg`, read, edit,
`bin/telora check`, and `bin/telora run` against the owned crate.

Read the staged Telora tutorial, ontology eDSL design, and role contract completely. Implement the
reusable eDSL only in `crates/ontology-edsl/`. Do not look for an enterprise domain, existing
ontology implementation, repository examples, tests, RFCs, or another agent's work.

If `requirement/FEEDBACK.md` exists, this is a bounded correction round. Read it completely and
change only what the feedback requires; preserve unrelated accepted behavior.

You may use only the local listing, search, read, edit, and staged Telora commands listed in the
role contract. Do not use network access, Cargo, another compiler, or another Telora binary.

Deliver the package source, `EDSL_TUTORIAL.md`, `AI3_CONTRACT.md`, `STAGE2_DESIGN.md`, and
`STAGE2_NOTES.md` under the owned crate. Record exact staged Telora checks, design limits, typed
adapter costs, and remaining risks. Report a concise completion summary without pasting source.
