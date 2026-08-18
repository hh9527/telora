# RFC 0251: Typed Semantic Equality

- Status: Implemented
- Tracking issue: #98
- Depends on: RFC 0070, RFC 0074, RFC 0095, RFC 0221, RFC 0248, RFC 0250

## Summary

Telora equality becomes a same-type operation:

```text
== : for(T) Fn(T, T) -> Bool
!= : for(T) Fn(T, T) -> Bool
```

`std/eq.equal` has the same signature and reaches the same runtime primitive.
No identity operators (`===` or `!==`) are introduced.

This RFC replaces the heterogeneous-equality rules in RFC 0074, RFC 0095, and
RFC 0221. Their structural value comparison, Function identity, finite Float,
and exact-complement rules remain in force for well-typed operands.

## Motivation

The existing `for(A, B) Fn(A, B) -> Bool` contract lets unrelated known types
reach runtime equality and return `False`. It also contains a runtime exception
that compares an exact nominal Atom witness with an unbranded Atom by payload.
That exception cannot distinguish an authored literal, which may legitimately
receive contextual nominal type, from an Atom obtained through `Any`, Union, or
another dynamic computation.

Canonical `TypeId` and `Val.ty` now provide an exact nominal boundary. Equality
must preserve that boundary and let the frontend reject statically unrelated
types. Runtime comparison remains total for values that cross an explicitly
dynamic boundary, but it never guesses provenance or brands a raw value.

## Static semantics

Both operands introduce one shared type variable `T`. Bidirectional checking
solves both operands against that variable and rejects incompatible evidence at
the operator or function-call source location.

Known primitive, container, structural, and nominal mismatches are errors. Two
exact nominal operands must have the same canonical `TypeId`; equal underlying
structure is insufficient. `Array(Int) == Array(String)` and distinct nominal
struct or enum declarations are therefore rejected before execution.

Authored Dict, Atom, and Tagged literals retain bidirectional contextualization.
When the other operand supplies an exact nominal type, the literal is checked
against that type regardless of operand order:

```telora
enabled == 'True
'True == enabled
```

Both forms have the same static type and runtime result. Contextualization is an
elaboration property; runtime equality does not rebrand values.

`Any`, Union, and other explicitly dynamic static domains may contain different
runtime variants. Comparing two values admitted by the same static domain is
well typed even when their runtime representations differ.

## Runtime semantics

Runtime equality first observes exact nominal witnesses:

1. equal nonzero canonical `TypeId` witnesses proceed to payload comparison;
2. unequal canonical witnesses return `False` at a dynamic boundary;
3. one witnessed and one raw value return `False`;
4. two raw values proceed by logical meta domain and payload.

The previous nominal/raw Atom exception is removed.

Payload comparison retains the existing rules:

- Int, finite Float, String, Atom, and native scalar domains compare by value;
- Array, Tuple, Dict, Tagged, and declared payloads compare recursively;
- Function values compare by opaque function identity;
- Dyn and Opaque retain their established logical identity contracts;
- different runtime meta domains return `False`;
- source location, provenance, Heap storage owner, and future `narrow` metadata
  do not participate; and
- Fail is not a value and propagates without producing a Boolean result.

Two values with the same Union static type may carry different runtime variants;
that comparison returns `False`, not an internal error. A nominal Atom variant
and a raw Atom variant remain unequal even when their text is identical.

`!=` evaluates the same operands in the same order and is exactly the Boolean
complement of `==` for every successful comparison. It has no independent
comparison protocol.

## Standard library

`std/eq` publishes:

```telora
native equal: for(T) Fn(T, T) -> Bool;
export { equal };
```

`eq.equal(left, right)` and `left == right` share static acceptance and runtime
results. Pattern matching continues to use its internal literal-selection
operation; this RFC does not make pattern selection a public heterogeneous
equality escape hatch.

## Non-goals

- `===` or `!==`;
- observable Heap handles or object addresses;
- operator overloading or trait dispatch;
- implicit Int/Float conversion;
- structural equality across distinct nominal identities;
- changing established Function, Dyn, or Opaque logical identity; or
- using equality to refine a Union or prove match exhaustiveness.

## Implementation plan

1. Replace equality's two independent static operand constraints with one
   shared inference variable while preserving bidirectional literal context.
2. Change `std/eq.equal` to a single type parameter.
3. Remove the nominal/raw Atom exception from `HeapView::values_equal_with`.
4. Audit operator bytecode, the native function, pattern selection, reference
   interpreters, and dynamic boundaries for heterogeneous bypasses.
5. Update the language SSOT, tutorial, and historical RFC implementation notes.

## Acceptance criteria

1. `1 == "1"`, distinct nominal declarations, and incompatible containers are
   frontend errors with source locations.
2. Same-type scalars, structures, recursive values, Function identity, Dyn,
   and Opaque retain their expected results.
3. `nominal == literal` and `literal == nominal` both contextualize and agree.
4. A witnessed nominal Atom and an unbranded dynamic Atom are unequal.
5. Different runtime variants admitted by one Union compare `False`.
6. `eq.equal(a, b)` accepts and returns exactly what `a == b` does.
7. `!=` is the exact complement of successful `==` comparisons.
8. The SSOT documents only typed equality and no identity operator.
9. The complete workspace test suite passes.

## Implementation outcome

Implemented by the #98 branch. Equality inference now uses one semantic type,
`std/eq.equal` publishes the same single-parameter contract, and runtime value
comparison never crosses a nominal witness boundary by guessing payload
provenance. Pattern tag selection uses a separate internal operation.

Concrete declared witnesses are installed by the `OwnDeclared` bytecode
operation for every expression carrying an exact analyzed owner, including
field and control-flow results. Tool-stage evaluation consumes the same owner
map as normal module execution. This avoids both witness loss in generic
callbacks and the source-location rebasing that occurred when ownership was
previously encoded as an ordinary native call.
