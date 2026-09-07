# RFC 0091: Opaque `Dyn` values

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Dyn witnesses contain supported canonical types; Any metadata is rejected.
- Depends on: RFC 0089, RFC 0090
- Tracking issue: https://github.com/hh9527/forma/issues/4

## Summary

Forma adds one nominal opaque primitive type, `Dyn`, representing an
existentially packed type witness and matching value:

```text
Dyn = exists A. (TypeOf(A), A)
```

The standard module `@bim/std/dyn` initially exposes:

```forma
native pack: for(A) Fn(TypeOf(A), A) -> Dyn;
native desc: Fn(Dyn) -> Type;
native kind: Fn(Dyn) -> ValueKind;

native check_int: Fn(Dyn) -> Option(Int);
native check_float: Fn(Dyn) -> Option(Float);
native check_string: Fn(Dyn) -> Option(String);
native check_bytes: Fn(Dyn) -> Option(Bytes);
```

`pack` is public because its generic contract is itself the required proof:
the checker accepts the value only as the same `A` named by the witness.
Ordinary Forma code cannot directly construct, destructure, or mutate `Dyn`.

## Nominal boundary

`Dyn` is not an alias for `Any`, Dict, Tagged, Tuple, or Function. A structural
encoding would let ordinary code forge a descriptor/value mismatch. The VM
therefore stores an opaque pair and the static type system recognizes one
closed primitive descriptor.

The value may be copied, captured, passed, returned, stored in Arrays and
Dicts, and promoted across module boundaries. Its descriptor and payload remain
reachable for GC and publication but are not separately accessible except
through trusted module operations.

`Dyn` equality is opaque identity equality in this phase. Structural equality
of represented values is the job of an explicit interpreter, not the wrapper.
Debug output identifies a `Dyn` without printing or recursively exposing its
payload.

## Descriptor invariant

For every successfully constructed package:

```text
decode(desc) = A
value checks against A
```

`pack` receives a `TypeOf(A)` argument, so normal static calls already satisfy
the invariant. The native boundary also validates canonical metadata before
storing it, protecting calls that cross explicit `Any` or malformed bytecode.

`desc` returns the exact canonical metadata stored in the package. It may be a
`$ref` when a structural observer later returns a child package from recursive
metadata. RFC 0090 operations are authoritative for observing it.

## Value kinds

`ValueKind` is a closed public Enum describing logical runtime shapes:

```text
Int Float String Bytes Dict Array Atom Tagged Tuple Function Dyn
```

It does not include VM handles, up-links, registers, or storage generations.
`kind` reports the wrapped payload kind; explicit nested packing therefore
reports `'Dyn`.

## Checked primitive projection

`check_*` validates both the stored descriptor and runtime payload kind. A
matching primitive returns `'Some(value)`; every mismatch returns `'None`.
It never performs numeric conversion, metadata widening, or coercion.

`expect_* -> Result` is deferred until structural observers establish one
shared path and blame convention. `Option` is sufficient for interpreter branch
selection and keeps this RFC focused on the representation invariant.

Generic recovery is also deferred:

```forma
for(A) Fn(TypeOf(A), Dyn) -> Option(A)
```

It requires canonical equivalence and trusted recovery for every public data
shape. Concrete leaf projection is enough for the first equality interpreter.

## Goals

1. represent descriptor/value erasure without exposing mismatched states;
2. preserve the package through ordinary value storage and publication;
3. expose the exact descriptor and logical payload kind;
4. recover primitive leaves only after checked narrowing;
5. keep packing explicit and statically tied to `TypeOf(A)`; and
6. provide the erased input ABI required by RFC 0093.

## Non-goals

- a global `Unknown` top type or implicit conversion to `Dyn`;
- field, item, tag, or payload observation;
- generic `Dyn -> A` recovery;
- mutation or dynamic construction of represented data;
- structural equality, hashing, serialization, or display of payloads;
- runtime type generation; or
- cyclic runtime values.

## Acceptance criteria

1. `pack(Int, 1)` has static type `Dyn` and reports descriptor `'Int`;
2. `pack[Int](Int, "x")` is rejected statically;
3. `check_int(pack(Int, 1))` returns `'Some(1)`;
4. `check_string(pack(Int, 1))` returns `'None`;
5. Float, String, and Bytes projections obey the same exact rule;
6. wrapped payload kind is reported without exposing VM representation;
7. no source literal or ordinary data constructor can forge `Dyn`;
8. Dyn values survive closure capture, collection storage, and publication;
9. debug output is bounded and opaque; and
10. existing values, metadata, equality, GC, and publication do not regress.

## Implementation plan

1. add the closed `Dyn` type descriptor and canonical metadata kind;
2. add opaque persistent and runtime representations with GC/copy support;
3. add the safe generic `pack` native declaration and implementation;
4. expose descriptor, logical kind, and primitive checked projections;
5. define identity equality and opaque formatting;
6. add static, runtime, storage, publication, and malformed-boundary tests; and
7. run the full quality gate and record the implementation result.

## Implementation result

Implemented `Dyn` as a closed nominal descriptor plus an opaque heap object
containing identity, canonical descriptor, and payload edges. The object
participates in heap copying, promotion, closure capture, collection storage,
legacy Value import/export, bounded debug display, and identity equality. Its
descriptor and payload remain private at the public Value boundary.

`@bim/std/dyn` provides the generic `pack`, `desc`, `kind`, and exact Int,
Float, String, and Bytes `check_*` projections. `pack` validates canonical
runtime metadata in addition to its static `TypeOf(A), A` contract. Primitive
projection strips explicit `WithAttributes` wrappers and resolves metadata
references before checking both descriptor and payload kind.

`Dyn` was added to canonical TypeMetadata, analysis graphs, module interfaces,
validation, and codec planning as an opaque leaf. JSON and JSON Schema reject
it explicitly. No implicit conversion, top type, generic recovery, or
structural observation was added.

Regression coverage checks inference, explicit type mismatch rejection, all
primitive projections, descriptor and logical-kind observation, closure
capture, Array storage, publication, opaque display, and identity equality.
Full Forma tests pass with 283 passed and 1 ignored; all 13 CLI integration
tests pass.
