# RFC 0189: Diagnostic convenience intrinsics

- Status: Implemented
- Diagnostic emission and Result handling are now governed by
  [RFC 0279](0279-unified-result-diagnostics.md); this document records history.
- Depends on: RFC 0188

## Summary

Add four message-first, variadic convenience forms over the diagnostic
primitives established by RFCs 0187 and 0188:

```forma
emit_info!("using a deprecated field", field);
emit_warn!("the fallback is ambiguous", candidate_a, candidate_b);
emit_error!("the dimension is incompatible", measure, dimension);
fail!("no value can be lowered", value);
```

They lower before type checking as follows:

```forma
emit_info!(message, subjects...)
# report('Info, blame!(message, subjects...))

emit_warn!(message, subjects...)
# report('Warn, blame!(message, subjects...))

emit_error!(message, subjects...)
# report('Error, blame!(message, subjects...))

fail!(message, subjects...)
# raise!(blame!(message, subjects...))
```

The forms add no new runtime effect or VM instruction. In particular,
`emit_error!` returns the constructed BlameError just as `report` does, while
`fail!` has type Never.

## Evaluation boundary

Reporting permits ordinary Forma control flow to continue and discover more
independent problems. A reported Error still invalidates the final evaluation,
so no value computed after it can be published as a successful result.

This RFC does not add recovery inside calls to combinators such as `array.map`.
An invoked function that uses `raise!` still terminates that VM evaluation.
The existing workspace evaluator may separately evaluate independent retained
module bindings. Domain validation that can continue should use `emit_error!`
and an ordinary `Option` when a local lowered value is unavailable; `raise!`
is reserved for a dependency path that cannot continue.

## Acceptance criteria

1. all four forms accept one String message and zero or more heterogeneous
   subjects;
2. the three emit forms retain `report` severity and identity-return behavior;
3. `emit_error!` allows subsequent expressions to run but rejects final
   success;
4. `fail!` produces the same structured raised-blame failure as `raise!`;
5. subject and rule provenance remain available to Host diagnostics;
6. no diagnostic stream, handler, port, or partial value enters Forma's value
   world.

## Implementation result

The parser lowers the four contextual intrinsics into the existing canonical
blame record, report call, and Raise AST. Compiler and parser tests cover
arity, String message checking, variadic labels, warning identity return,
Error success invalidation, and fatal raising. No runtime or bytecode surface
was added.
