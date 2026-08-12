# Telora validation workflow

The workspace provides a fixed Telora validation interface. Run commands from
the workspace root.

## Layout

Place reusable library modules in `a2/src/`.

Use `a2/bin-src/main.telora` as the main validation entry. Use
`a2/bin-src/test.telora` as the focused scratch validation entry.

`a2/telora-deps.json` fixes the workspace dependency boundary. Read it when
needed. Do not modify it.

## Commands

The following commands are the complete executable interface available in the
experiment:

```text
./bin/run
./bin/run-test
./bin/types
./bin/show
```

- `./bin/run` evaluates `a2/bin-src/main.telora` and prints its exported
  `output` value.
- `./bin/run-test` evaluates `a2/bin-src/test.telora` and prints its exported
  `output` value.
- `./bin/types` prints the inferred types for
  `a2/bin-src/main.telora`. It is the module-level type summary: quantified
  definitions retain their `for(...)` schemes, and internal function
  parameters are not listed as module bindings.
- `./bin/show` prints the semantic snapshot for `a2/bin-src/main.telora`,
  including diagnostics, modules, definitions, references, expressions, and
  types. Generic definition rows retain their quantified schemes. Nested
  parameter and expression rows are uninstantiated debug facts; an `Any` on
  those rows does not replace the enclosing definition's displayed scheme.
Each command preserves Telora's standard output, standard error, and exit
status. A zero exit status means that the requested operation succeeded. A
nonzero exit status means that Telora or the wrapper rejected it; read the
diagnostic, revise the source, and run the relevant command again.

The wrappers accept no source paths or semantic-query positions. They do not
discover files, mutate source, or run multiple validation entries. Rewrite
`test.telora` when a behavior should be tested independently from
`main.telora`.
