# RFC 0167: Structured blame re-raise

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Runtime diagnostic emission uses message and subject registers rather than BlameError values.
- Depends on: RFC 0056, RFC 0107, RFC 0166

## Summary

Forma will add one narrow contextual intrinsic:

```forma
reraise!(error)
```

The operand must be a `BlameError` and the expression has type `Never`.
Evaluation terminates the current computation with the existing error rather
than constructing a new one. The error's message and its data/rule provenance
remain intact; the `reraise!` operation contributes only its ordinary runtime
trace frame.

## Motivation

User-space validation and interpreters return structured `BlameError` values.
At a strict Host boundary such as `ExecFn`, the computation must terminate
instead of returning `Result`. Converting the error with
`panic!(error.message)` discards the blamed data and authored rule locations,
so imported data is diagnosed at the propagation site rather than its source.

`reraise!` closes that specific boundary without making errors exceptions and
without changing Result-based composition inside ordinary Forma code.

## Semantics

`reraise!` accepts exactly one argument whose static type is the canonical
`BlameError` record:

```forma
@struct type BlameError = {
    message: String,
    data: Any,
    rule: Any,
};
```

The runtime reads the three fields from that value. `message` supplies the
diagnostic text, while the provenance carried by `data` and `rule` supplies
the primary data location and secondary contract-rule location. No field is
rewritten and no replacement `BlameError` is allocated.

The source location of `reraise!` remains visible through the normal runtime
trace. It must not replace either structured blame anchor.

## Boundary

This RFC does not add `throw`, `catch`, exception handlers, resumable effects,
or implicit error propagation. `BlameError` remains an ordinary value until a
program explicitly invokes `reraise!`. The intrinsic is terminal and cannot
be intercepted by Forma code.

`panic!` remains the operation for constructing a new String-based failure at
the current source position. It is not an alias for `reraise!`.

## Acceptance criteria

1. `reraise!` accepts exactly one `BlameError` and has type `Never`;
2. wrong arity and non-`BlameError` operands fail during frontend analysis;
3. runtime failure preserves the original message plus distinct data and rule
   provenance;
4. the propagation site is retained in the runtime trace without replacing
   either provenance anchor;
5. the GCC-wrapper fixture uses `reraise!` at its strict validation boundary;
6. malformed imported JSON names both `source.json` and the authored rule in
   `toolchain.forma`;
7. successful execution and existing `panic!` behavior remain unchanged;
8. full workspace tests, formatting, and warning-denied Clippy pass.

## Implementation plan

1. lower `reraise!` to a dedicated typed AST expression;
2. carry it through LIR and bytecode as a dedicated terminal operation;
3. reconstruct a runtime failure from the structured value and its rich-value
   provenance;
4. replace lossy validation panics in the GCC-wrapper fixture and record the
   end-to-end diagnostic evidence.

## Stopping rules

Work returns to discussion if implementation requires catchable exceptions,
implicit Result conversion, a general effect system, fabricated source
locations, or changing the public shape of `BlameError`.

## Implementation result

Implemented in August 2026. `reraise!` lowers to a dedicated AST, LIR, and
bytecode operation. Static analysis requires canonical `BlameError` and gives
the expression type `Never`; the VM independently verifies the record and
String message before constructing a terminal `ReraisedBlame` runtime failure.

The VM takes diagnostic anchors from the rich `data` and `rule` fields and
keeps the opcode origin in the ordinary trace. Focused tests cover arity,
operand type, message preservation, distinct anchors, and trace presence. The
GCC-wrapper fixture uses the intrinsic for validation and argv failures; its
malformed dependency data test observes both `source.json` and
`toolchain.forma`. Existing `panic!` behavior is unchanged.
