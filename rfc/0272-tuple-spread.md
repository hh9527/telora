# RFC 0272: Tuple Spread

- Status: Accepted
- Tracking: [#159](https://github.com/hh9527/telora/issues/159)
- Baseline: `4faf8e7`

## Motivation

Tuples represent fixed-length heterogeneous products. Tuple spread composes
these products without repeated projections or repeated receiver evaluation.

## Syntax and Semantics

```telora
let a = (1, "hi");
let b = (...a, 'True.ty!(Bool), 3);
```

The result type is `Tuple([Int, String, Bool, Int])`. Spread is accepted only
as a tuple literal item, alongside ordinary expressions. Multiple spreads and
trailing commas are supported. `(...a)` and `(...a,)` both construct a Tuple;
ordinary `(a)` remains grouping and `(a,)` remains a singleton tuple.

Each spread operand must have a statically known Tuple type, including tuple
contracts containing generic element types. Array, Dict, Dyn and unconstrained
type variables do not provide a fixed tuple shape. Empty tuples contribute no
elements. Expansion is one level; a tuple-valued ordinary item remains nested.

Each output position retains its own type. No homogeneous common-type inference
or synthetic enum is involved. A target Tuple contract supplies context by the
expanded index, including to directly authored literals inside spread tuple
literals. Existing values retain their own nominal identities. The complete
expanded length and all element types must match the expected tuple contract.
Opaque expressions used as spread operands must establish their tuple shape
independently; an expected suffix does not guess their arity.

Operands and ordinary items are evaluated once, from left to right. Copying
spread elements preserves Val provenance and nested type identity. The new
tuple receives the tuple construction expression's location. Empty spreads
still evaluate their operand. Failure propagation and allocation quotas follow
the existing collection construction rules.

## Implementation

Reuse Spread AST items in tuple syntax. Flatten tuple types in preliminary and
strict inference, and align contextual literal checking with expanded positions.
Compile ordered tuple fragments and concatenate them using a dedicated tuple
instruction; literals without spread retain direct MakeTuple construction.

## Boundaries

This does not add variadic functions, argument spreading, rest patterns,
Array-to-Tuple conversion or inference of unknown tuple arity. Array and Dict
spread retain their current contracts.

## Acceptance

Pure Telora cases cover heterogeneous composition, multiple/empty/single
spreads, nested tuple preservation, generic functions, expected nominal element
context and comparisons. Rejection cases cover invalid operands, unknown arity,
expanded length/type mismatch and mismatched nominal elements. Query and
diagnostic fixtures verify flattened types, copied provenance and operand
evaluation order/count. Run debug builds and workspace tests, no release build.
