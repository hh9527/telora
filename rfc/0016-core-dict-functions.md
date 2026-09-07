# RFC 0016: Core Dict Functions

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Dict helper contracts use homogeneous Dict(A) with preserved element types.

## Summary

XL gains an explicit built-in Dict module:

```xl
import dicts from "core:dict";
```

It exports five pure functions:

```text
keys(dict)
values(dict)
pairs(dict)
from_pairs(pairs)
merge(left, right)
```

The functions preserve XL's canonical field ordering and operate directly on
runtime heap values. This RFC does not add methods, mutable Dicts, callbacks,
general iterators, or recursive/deep merge semantics.

## Motivation

Imported JSON and computed metadata frequently need to cross the boundary
between named records and collection pipelines. RFC 0015 provides the Array
side of that composition, but XL cannot yet enumerate a Dict, transform its
entries, or assemble a Dict from computed pairs without dedicated syntax.

An explicit `core:dict` module keeps these operations ordinary imported values
and composes with `core:array` without adding configuration-specific sugar:

```xl
import arrays from "core:array";
import dicts from "core:dict";

dict
    |> dicts.pairs()
    |> arrays.map(fn(pair) { pair })
    |> dicts.from_pairs()
```

## Core module identity

`core:dict` is reserved and resolved before filesystem paths. It cannot be
shadowed by a file. The module is published once into the module loader's
persistent world and exports exactly `keys`, `values`, `pairs`, `from_pairs`,
and `merge`.

The imported namespace is conventionally lowercase so it remains distinct
from the existing `Dict` value/type terminology.

## Canonical order

XL Dict shapes contain unique field names sorted by UTF-8 string order. All
enumeration functions expose that canonical order, independent of source or
insertion order:

- `keys(dict)` returns field names in shape order;
- `values(dict)` returns values in the corresponding shape order;
- `pairs(dict)` returns `(key, value)` Tuples in the same order.

`from_pairs` and `merge` construct a canonical shape, so enumerating their
results follows the same rule. No insertion-order contract is introduced.

## API

### `keys`

```text
keys(dict: Dict) -> Array(String)
```

Returns a newly allocated Array of String field names. A non-Dict argument is
a type mismatch.

### `values`

```text
values(dict: Dict) -> Array(Any)
```

Returns a newly allocated Array containing the original field value references
in canonical field order. It does not deep-copy values.

### `pairs`

```text
pairs(dict: Dict) -> Array(Tuple(String, Any))
```

Returns a newly allocated Array of newly allocated two-element Tuples. Each
Tuple contains a field-name String followed by the original value reference.

### `from_pairs`

```text
from_pairs(pairs: Array(Tuple(String, Any))) -> Dict
```

Each input item must be a Tuple of exactly two elements and its first element
must be a String. Atom keys are not accepted. Duplicate field names are an
error rather than an implicit overwrite. The output shape is sorted
canonically; input order is otherwise irrelevant.

### `merge`

```text
merge(left: Dict, right: Dict) -> Dict
```

Returns a shallow merge. Fields present only on one side retain that side's
value reference. When both Dicts contain the same field, the right value wins.
Nested Dicts are values and are not recursively merged.

`merge` always returns a new Dict value, including when either input is empty.
This keeps allocation and identity behavior uniform and leaves structural
sharing optimizations as implementation details.

## Runtime boundary

These operations require no XL callbacks and no native continuation. They are
trusted VM-managed core functions so they can read runtime handles directly
and allocate results in the current local heap without exporting through the
legacy `Value` boundary.

The implementation reads Dict shapes and value slices together from one
`HeapView`. Persistent input values may be referenced by local result objects;
the background heap remains read-only. Shape order permits linear enumeration
and a linear two-way merge without repeated textual lookup.

## Fuel and allocation quotas

Each function call consumes the ordinary single call fuel unit. Traversal does
not add per-field fuel, consistent with RFC 0010 and RFC 0015: fuel measures
control-flow progress rather than virtual CPU time.

All result storage is charged before allocation:

- Array element slots for `keys`, `values`, and `pairs`;
- each pair Tuple's two value slots;
- String bytes when field names are materialized in the local heap;
- Dict value slots and field-name bytes for `from_pairs` and `merge`.

Existing references copied into a result are not deep-copied and do not incur
the allocation cost of their reachable graphs. A quota failure does not expose
a partial result.

## Static analysis

The focused checker exposes the exact core-module Dict shape and function
arities. Precise generic relationships remain `Any`, matching `core:array`.
Closed tool-stage metadata computations may use all five functions through the
ordinary module initialization VM and quota.

## Diagnostics

Errors identify the core function and retain the XL call origin. They include:

- non-Dict arguments to `keys`, `values`, `pairs`, or either side of `merge`;
- non-Array input to `from_pairs`;
- an item that is not a two-element Tuple;
- a pair key that is not a String;
- a duplicate field name in `from_pairs`;
- allocation quota exhaustion or representational size overflow.

## Rejected alternatives

### Preserve insertion order

Dict identity is already a canonical sorted shape. Adding an independent
insertion order would complicate equality, interning, promotion, and output
without benefiting deterministic configuration evaluation.

### Let the last duplicate pair win

Unlike `merge`, `from_pairs` has no explicit precedence boundary. Rejecting
duplicates catches accidental collisions after Array transformations. Callers
that want overwrite semantics can construct separate Dicts and merge them.

### Deep merge nested Dicts

Deep merge requires policy for Arrays, type mismatches, deletion, and conflict
resolution. The primitive operation is deliberately shallow; richer policies
can be ordinary XL functions once the required collection primitives exist.

### Expose methods on Dict values

XL has no nominal method-dispatch model. An explicit module is consistent with
`core:array` and keeps capability origin visible.

## Deferred work

- precise generic static signatures;
- deep merge and user-defined conflict policies;
- field filtering, mapping, removal, and update functions;
- a general iterable or collection protocol;
- core module manifests, versions, and dependency reporting;
- allocation-free views or iterator fusion.

## Implementation plan

1. Generalize core-module registration so `core:array` and `core:dict` share
   one reserved resolution path and persistent cache.
2. Add the five fixed-arity Dict core-function identities.
3. Expose an internal Dict shape/value view and local allocation helpers without
   crossing the legacy `Value` boundary.
4. Implement canonical enumeration, validated pair construction, and linear
   right-biased shallow merge.
5. Add runtime, tool-stage, quota, ordering, error, and module-cache tests.

## Acceptance criteria

1. `core:dict` resolves without filesystem access and exports exactly the five
   specified functions.
2. `keys`, `values`, and `pairs` return corresponding data in canonical field
   order for empty and non-empty Dicts.
3. `from_pairs` accepts valid String-keyed pairs, canonicalizes order, and
   rejects malformed or duplicate entries.
4. `merge` is shallow, deterministic, and right-biased on collisions.
5. Operations retain runtime references rather than deep-exporting values.
6. Calls use one ordinary fuel unit and enforce exact allocation quotas without
   exposing partial results.
7. Closed tool-stage expressions can use `core:dict`.
8. Boundary failures retain the XL call origin and core function identity.
9. Existing VM, heap, module, Array core, quota, and CLI tests remain unchanged.

## Implementation result

`core:dict` is registered through the same reserved-module path and persistent
world cache as `core:array`. The five functions execute as trusted synchronous
core operations inside the ordinary VM call dispatcher. They inspect Dict
shape and value slices through `HeapView` and never cross the legacy `Value`
export boundary.

Enumeration follows canonical shape order. `pairs` creates two-element Tuples,
`from_pairs` validates every item and rejects duplicate String keys before
allocation, and `merge` performs a linear two-way shallow merge with right-side
precedence. Existing nested values remain shared runtime references.

Allocation is computed and charged before result objects are installed. Tests
cover canonical and empty results, round trips, shallow right-biased merge,
tool-stage metadata evaluation, exact output quota, malformed items, invalid
argument kinds, duplicate fields, and source-positioned failures. As planned,
generic static signatures and core-module dependency reporting remain deferred.
