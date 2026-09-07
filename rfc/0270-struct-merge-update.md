# RFC 0270: Struct Merge-Update

- Status: Accepted
- Tracking: [#157](https://github.com/hh9527/telora/issues/157)
- Scope: Nominal struct updates, including spreads inside update literals.

## Motivation

Updating a few fields should preserve a struct's nominal identity without
repeating all of its fields. Combining update values should not require an
anonymous struct type or an implicit conversion between nominal types.

## Syntax and Semantics

```telora
type A = struct { f1: Int, f2: String };
type B = struct { f2: String };
let a: A = { f1: 1, f2: "a" };
let b: B = { f2: "hi" };
let c = a & b & { f1: 2, ...b };
```

`c` has type A and fields `f1 = 2`, `f2 = "hi"`. The `&` token retains its
existing precedence and left associativity. Integer bitwise AND is unchanged.
Struct update is selected by the left operand's known nominal struct type.
An outer expected type does not supply a missing left operand identity.

The right operand is either a nominal struct value or an update literal.
Its field names must be a subset of the left type's fields. Its nominal
identity does not determine the result identity. Each final field value must
be assignable to the corresponding target field type, retaining ordinary
nominal checks for nested values. Missing update fields preserve the base.
Updates are shallow and produce a new value.

An update literal is contextual syntax, not an independently constructed
anonymous struct or Partial type. Explicit field expressions receive their
target field's expected type when they are the winning entry. Spread operands
must be nominal structs with statically known field sets; Dict, Dyn, enums and
unconstrained type parameters are rejected. Named generic struct instantiations
retain their full type arguments; this introduces no row polymorphism.

All contributed names, even overwritten ones, must belong to the target.
Entries merge left to right and later entries win. Only winning entries must
match target field types, but overwritten expressions must still type-check
independently. Already typed values are never reinterpreted by context.
Duplicate explicit field names are rejected, including across spreads, as in
existing Dict literals. Empty updates are allowed.

Each binary update is checked independently: `a & b & { x: 1 }` rejects an
incompatible `b.x` even if the following update overwrites it.

The base is evaluated first, then right-side expressions once in source order.
Overwritten expressions are evaluated too. Copied fields retain their Val
locations and nested identities; the new container receives the update
expression's location and the base's runtime type identity. Normal failures
remain observable, with existing resource accounting applied to allocations.

## Boundaries

Standalone struct spread remains deferred, including `{ ...a }` with a target
annotation. Existing Dict spread is unchanged. No anonymous struct type,
implicit nominal conversion, structural type search, new construction syntax,
deep merge, or generic field-subset constraint is introduced.

## Implementation and Validation

The parser already represents `&` and spread entries. Extend preliminary type
evidence and strict inference with the same left-identity rule, contextually
check update fields, and retain nominal identity during runtime merging.
The existing merge machinery can preserve copied field provenance.

Use primarily Telora acceptance cases for nominal identity, disjoint patches,
overrides, contextual nested construction, generic instances, chaining,
integer AND, empty updates, and field provenance. Rejection cases cover extra
fields, wrong types, invalid operands, standalone spread, duplicate explicit
fields, and intermediate invalid updates. Run debug builds and workspace tests.
