# RFC 0101: Contextual intrinsic syntax

- Status: Implemented
- Partial supersession by [RFC 0279](0279-unified-result-diagnostics.md):
  function-specific Result macros are replaced by unwrap! and ok_or_warn!.
- Depends on: RFC 0097, RFC 0098, RFC 0100

## Summary

Forma introduces a closed class of contextual intrinsics spelled:

```forma
name!(arguments)
```

Contextual intrinsics are compiler-recognized expressions that may inspect
authored source context or an expected static type. They are not Function
values and do not introduce a macro system. This RFC migrates the existing
interpreter lift from:

```forma
interpreter(show_dyn)
```

to:

```forma
interpreter!(show_dyn)
```

The old spelling is removed without a compatibility period. `blame!`, `file!`,
and `line!` are reserved for later RFCs but are not implemented here.

## Motivation

`interpreter` currently resembles an ordinary call even though it cannot be
bound, passed, imported, shadowed, or evaluated without an explicit expected
generic scheme. Future source-aware blame has the same mismatch: capturing the
authored rule site cannot be expressed by an ordinary Function receiving only
runtime arguments.

The `!` makes this contextual behavior visible at the use site. It also creates
one consistent surface for future context-dependent operations without
granting token manipulation or compile-time execution.

## Syntax

The expression grammar adds:

```text
contextual_intrinsic := intrinsic_name '!' '(' arguments? ')'
```

`intrinsic_name` is recognized by the compiler's closed intrinsic table. For
this RFC, only `interpreter` is enabled. The names `blame`, `file`, and `line`
are reserved and receive a dedicated "reserved but not implemented"
diagnostic. Any other `name!(...)` receives an unknown-contextual-intrinsic
diagnostic; it is not interpreted as a macro invocation.

Arguments are ordinary Forma expressions separated by commas. They do not form
a token tree and are parsed, recovered, indexed, and formatted using normal
expression rules. Postfix call, field, type-application, and section syntax may
consume the result of an intrinsic when its result type permits it.

## Semantic representation

The authored AST stores a dedicated contextual-intrinsic node containing:

1. the intrinsic kind;
2. the ordinary parsed argument expressions; and
3. the full authored source location.

An intrinsic kind is an internal closed enum, not a resolved Forma identifier.
HIR indexes references inside arguments but does not emit a reference for the
intrinsic name. Hover and navigation may describe a known intrinsic, but cannot
resolve it to a Function declaration.

Each enabled intrinsic owns its arity and contextual validation. There is no
general evaluation rule for `name!(...)`, no intrinsic registry visible to
Forma code, and no runtime callable created by the syntax.

## `interpreter!` migration

`interpreter!(operand)` preserves the complete RFC 0097 and RFC 0098 semantics:

- it is valid only as the direct initializer of an explicitly contracted
  generic `def`;
- semantic analysis derives the erased operand ABI from the expected scheme;
- HIR and tooling traverse the authored operand;
- compilation consumes the validated ordinary adapter elaboration; and
- no interpreter opcode or runtime callable is introduced.

The AST may retain the existing specialized Interpreter representation during
this implementation if the parser first recognizes it through the contextual
intrinsic syntax and diagnostics. A later unified intrinsic node is an internal
refactor, not a semantic requirement.

`interpreter(operand)` no longer parses as the special form. Because
`interpreter` remains reserved, the parser reports a focused migration message
rather than resolving it as a Function call.

## Reserved intrinsics

This RFC reserves these spellings:

```forma
blame!(...)
file!()
line!()
```

Reservation prevents user code and tooling from assigning accidental macro
semantics to them. It does not define their signatures or lowerings. RFC 0105
will define `blame!`; `file!` and `line!` remain future work outside the RFC
0100 phase.

A future `file!()` must expose a canonical resolved module identity rather than
a physical path. A future `line!()` uses the one-based authored source line.
These constraints reserve safe behavior without committing to implementation.

## Diagnostics and recovery

Diagnostics distinguish:

- old `interpreter(...)` spelling and its required `interpreter!(...)`
  replacement;
- unknown contextual intrinsic names;
- reserved but unavailable intrinsic names;
- wrong intrinsic arity;
- malformed argument syntax; and
- the existing invalid interpreter context or operand ABI errors.

Recoverable parsing retains the contextual expression and all recoverable
arguments. A missing closing parenthesis does not invent a callable or hide
references in already parsed arguments. Formatter and CST round trips preserve
the `!` exactly.

## Goals

1. visibly distinguish contextual language operations from Function calls;
2. migrate interpreter lifting to `interpreter!(...)` everywhere;
3. establish one closed syntax family for expected-type and source-site access;
4. retain authored arguments for HIR, diagnostics, formatting, and navigation;
5. provide precise unknown, reserved, arity, and migration diagnostics; and
6. preserve interpreter typing, elaboration, and runtime behavior.

## Non-goals

- user-defined macros or contextual intrinsics;
- token trees, hygiene, quasiquotation, or syntax objects;
- compile-time execution or arbitrary code generation;
- treating intrinsic names as values, imports, or resolved identifiers;
- implementing `blame!`, `file!`, or `line!`;
- changing interpreter scheme validation or parameter-wise lifting; or
- retaining `interpreter(...)` as an alias.

## Acceptance criteria

1. `interpreter!(operand)` parses into authored semantic structure;
2. valid existing interpreter programs behave identically after migration;
3. `interpreter(operand)` fails with a focused migration diagnostic;
4. unknown `name!(...)` forms receive a contextual-intrinsic diagnostic;
5. `blame!`, `file!`, and `line!` report that they are reserved but unavailable;
6. missing or extra interpreter arguments receive an arity diagnostic;
7. HIR indexes argument references but not the intrinsic name;
8. CST and formatting preserve the authored `!` and argument syntax;
9. hover, diagnostics, and module interfaces expose no generated adapter names;
10. all repository examples and current user documentation use the new spelling;
11. historical implemented RFCs remain unchanged as design records; and
12. full Forma, CLI, LSP, formatting, and strict static checks pass.

## Implementation plan

1. add `!` tokenization and contextual-intrinsic grammar/recovery;
2. classify enabled, reserved, and unknown intrinsic names in lowering;
3. route `interpreter!` through the existing semantic Interpreter node;
4. add the old-spelling migration diagnostic;
5. update examples, tests, README, VISION, and non-historical current docs;
6. add parser, CST, HIR, type, execution, recovery, and diagnostic tests; and
7. run the full quality gate and record the implementation result.

## Stopping rules

Work returns to discussion if implementation requires:

1. token trees, hygiene, expansion ordering, or user-defined syntax;
2. a first-class intrinsic or macro value;
3. runtime lookup or dispatch by intrinsic name;
4. changing the accepted interpreter scheme;
5. implementing source provenance or blame ahead of RFC 0102 and RFC 0105; or
6. accepting both interpreter spellings indefinitely.

## Implementation result

Forma now tokenizes `!` and parses contextual intrinsics through explicit
two-token grammar predicates, keeping ordinary identifiers and calls
unambiguous. `interpreter!(operand)` routes through the existing authored
Interpreter AST and parameter-wise semantic elaboration, so HIR visibility,
type validation, generated adapter isolation, and runtime behavior are
unchanged.

The old `interpreter(operand)` form remains only as a parser migration branch
and reports its exact replacement. Unknown names receive a closed-intrinsic
diagnostic; `blame!`, `file!`, and `line!` receive distinct reserved-but-not-yet-
implemented diagnostics. Interpreter arity is checked before elaboration, and
the lossless CST retains the authored bang and ordinary expression arguments.

All current examples, README variants, and VISION use `interpreter!(...)`.
Historical RFCs retain their original spelling as design records. Full Forma
tests pass with 293 passed and 1 ignored; all 13 CLI and 20 LSP tests pass, and
strict workspace Clippy reports no warnings.
