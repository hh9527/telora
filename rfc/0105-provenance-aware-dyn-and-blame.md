# RFC 0105: Provenance-aware Dyn and `blame!`

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Error subjects use producer-specific contracts; direct diagnostic emission observes internal Val subjects.
- Depends on: RFC 0089 through RFC 0099, RFC 0101, RFC 0102

## Summary

Forma completes the user-space interpreter diagnostic boundary in two parts:

1. Dyn packing and every structural observer preserve descriptor, payload, and
   payload provenance together; and
2. the contextual intrinsic:

```forma
blame!(data, message)
```

constructs a `BlameError` whose data anchor comes from `data` provenance and
whose rule anchor is the authored `blame!` site.

`blame!` returns the ordinary BlameError record. Callers explicitly place it in
`Result.Err` or another program-defined validation structure:

```forma
'Err(blame!(value, "unsupported descriptor"))
```

The intrinsic is source-contextual but has no runtime authority, hidden stack
inspection, or macro behavior.

## Motivation

RFC 0099 proved that a user-space interpreter can recursively observe Dyn and
return BlameError. Its example currently builds an ordinary record:

```forma
{data: value, rule: "my_show", message: message}
```

This retains the data location but the rule String is ordinary data and does
not reliably identify the authored validation rule. A fake ordinary
`blame.make` Function cannot capture its caller's location. Native codec and Dyn
errors already carry separate data and rule locations; user-space interpreters
need the same quality without gaining Host reflection.

## Dyn invariant

A Dyn package remains an existential pair:

```text
Dyn = exists A. (TypeOf(A), A)
```

At runtime, both stored edges are RichValues and independently retain RFC 0102
provenance. Packing:

```text
pack_dyn(witness, value)
```

preserves witness provenance on the descriptor edge and value provenance on the
payload edge. The Dyn root follows ordinary generated-call rebasing, but root
rebasing never rewrites either child.

Structural observers advance descriptor and payload in lockstep. A child Dyn
uses the selected child descriptor and selected child value, with the value
edge's provenance as its diagnostic anchor. Arrays of children and named field
pairs may have generated container roots while every child retains its own
provenance.

No observer may return a child descriptor paired with its parent value, copy a
parent location over a selected child, or turn descriptor provenance into data
provenance.

## `blame!` syntax and result

RFC 0101's closed contextual-intrinsic syntax enables exactly two arguments:

```text
blame!(data, message)
```

It lowers to the ordinary canonical BlameError shape:

```forma
{
    data: data,
    message: message,
    rule: <authored blame! marker>,
}
```

The marker is an opaque ordinary value for BlameError's existing `rule: Any`
field. In this phase its printable value is the stable String `"blame!"`, but
its RichValue provenance is Generated at the full intrinsic call site. Code
must not parse the marker String to recover a location; Host diagnostics use
its provenance.

The `data` field retains its existing value and provenance. The message must
check as String. The constructed record root is Generated at the intrinsic
site. No source path, line number, or Location object becomes a Forma value.

## Lowering

`blame!` is represented by authored contextual syntax in the CST and lowers to
an ordinary Dict expression with authored data/message children and a synthetic
rule-marker child derived from the intrinsic location. HIR sees references in
data and message, but no reference for `blame` and no generated binding.

The ordinary Dict type checker proves compatibility with BlameError. The
ordinary compiler and VM construct the record; no bytecode opcode, native
callback, or runtime intrinsic dispatch is introduced.

This bounded lowering is safe because the only contextual capability is
attaching the already-known authored source location to a compiler-created
constant. It cannot inspect the caller stack or arbitrary source database.

## Diagnostic consumption

Existing native Result/codec/validation paths read BlameError fields and use:

- `error.data` provenance as the primary data anchor;
- `error.rule` provenance as the authored rule anchor; and
- `error.message` as the diagnostic text.

When data and rule differ, rendering produces a primary data label and a
secondary rule label. Generated data naturally points at its generation or
rebased call site. Imported and selected data points into JSON, TOML, or YAML.

Constructing a BlameError is still ordinary pure computation. It does not emit
a Host diagnostic, create Never, abort evaluation, or implicitly return Err.

## Reference interpreters

The reference Show and Equality interpreters migrate their helper logic to:

```forma
def blame: Fn(Dyn, String) -> ShowResult = fn(value, message) {
    'Err(blame!(value, message))
};
```

This demonstrates that recursive user-space interpretation can preserve a
deep imported child as the data anchor while identifying the exact authored
rule site. The native equality operator and standard equality Function remain
unchanged.

## Goals

1. preserve selected child provenance through every Dyn observer;
2. give user-space interpreters a safe authored rule anchor;
3. construct the existing BlameError rather than another error hierarchy;
4. keep diagnostic emission and failure policy under Host control;
5. avoid runtime intrinsics, stack inspection, or location reflection;
6. migrate the reference Show and Equality examples; and
7. provide the blame bridge used by RFC 0106 end-to-end diagnostics.

## Non-goals

- exposing Location, source paths, provenance kinds, or call stacks to Forma;
- changing Dyn's static or runtime safety boundary;
- unpacking Dyn into a statically recovered A;
- implicitly wrapping BlameError in Result.Err;
- emitting a diagnostic or Never from `blame!`;
- adding `file!()` or `line!()`;
- user-defined contextual intrinsics or macros;
- changing native equality, Show, codecs, or validation policy; or
- adding a new VM opcode/native Function.

## Acceptance criteria

1. `blame!(data, message)` checks as the canonical BlameError shape;
2. zero, one, or more than two arguments receive a dedicated arity diagnostic;
3. a non-String message receives an ordinary precise type diagnostic;
4. data value and provenance are preserved without copying or relabeling;
5. rule marker provenance is the full authored `blame!` call site;
6. the record root is Generated at the intrinsic site;
7. Dyn pack preserves independent descriptor and payload provenance;
8. field, array, tuple, tag, and payload observers retain selected child data
   provenance through returned Dyn packages;
9. observer container roots do not overwrite child provenance;
10. HIR indexes authored arguments but not the intrinsic name;
11. reference Show and Equality use `blame!` and retain behavior;
12. a rendered failure shows distinct imported-data and authored-rule anchors;
13. no Location value, opcode, native callback, macro facility, or diagnostic
    side effect is introduced; and
14. full Forma, CLI, LSP, formatting, and strict static checks pass.

## Implementation plan

1. enable `blame!` in contextual-intrinsic lowering with exact arity;
2. lower to canonical Dict data/message/rule fields with a sourced rule marker;
3. audit Dyn pack and every structural observer against RFC 0102 provenance;
4. add parser, CST, HIR, typing, runtime-location, nested-observer, and rendered
   cross-source diagnostic tests;
5. migrate reference Show and Equality helpers;
6. keep `file!` and `line!` reserved; and
7. run the full quality gate and record the implementation result.

## Stopping rules

Work returns to discussion if implementation requires:

1. exposing source locations or paths as Forma values;
2. stack inspection or dynamic caller discovery;
3. a new error type instead of BlameError;
4. implicit Result control flow or Host diagnostic emission;
5. unchecked Dyn casts or descriptor/value mismatch;
6. a VM opcode, runtime intrinsic registry, or user macro system; or
7. recursively relabeling child provenance at the blame site.

## Implementation result

Implemented without invoking a stopping rule.

`blame!(data, message)` is accepted by the closed contextual-intrinsic parser
and lowers directly to the canonical `{data, message, rule}` Dict. The rule
marker is the String `"blame!"` sourced at the complete authored invocation;
the data expression remains untouched, and ordinary Dict typing enforces a
String message and compatibility with BlameError. Exact arity, reserved
`file!`/`line!`, CST round trips, HIR reference indexing, and type errors have
focused coverage. No opcode, native callback, source-reflection value, or
diagnostic side effect was added.

The Dyn audit found and fixed a provenance-kind loss: packing and structural
observers previously rebuilt wrappers from `value.loc()`, which preserved the
location but converted Original provenance into Generated provenance. Dyn
wrappers now replace only the RuntimeValue while inheriting the payload's full
RichValue provenance. Field, array, tuple, tag, and payload observers therefore
retain the selected child's diagnostic identity through nested interpretation.

Reference Show and Equality now construct failures with `blame!`. An
end-to-end test imports JSON, passes it through `interpreter!`, Dyn field
observation, `blame!`, and `result.unwrap`, and confirms that rendering anchors
the data label in the JSON source and the rule label at the authored intrinsic.

The full workspace gate passes: 313 Forma library tests passed with 1 ignored,
13 CLI tests passed, 20 LSP tests passed, all documentation tests passed, and
Clippy completed for every workspace target with warnings denied.
