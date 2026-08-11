# RFC 0220: Array Access and Enumeration

- Status: Implemented
- Depends on: RFC 0011, RFC 0012, RFC 0015, RFC 0021, RFC 0023, RFC 0048,
  RFC 0053, RFC 0127, RFC 0219
- Tracking issue: #18

## Summary

Extend `std/array` with two pure, typed operations:

```telora
get:       for(A) Fn(Array(A), Int) -> Option(A)
enumerate: for(A) Fn(Array(A)) -> Array(Tuple([Int, A]))
```

`get` performs zero-based, total access: negative and out-of-range indices
return `'None`. `enumerate` returns each source value paired with its zero-based
`Int` index in source order.

Both operations use the existing core Array function boundary. They add no
indexing syntax, iterator protocol, callback continuation, mutation, or new VM
instruction.

## Motivation

Telora's Array module has deterministic traversal and composition operations,
but it cannot directly retrieve a known position or retain positions while
transforming a collection. Programs can sometimes restructure an algorithm
around `fold`, `filter`, or catalog order, but that changes the model solely to
compensate for a small standard-library gap.

The opencode test-10 A2 experiment exposed this gap while implementing bounded
path traversal. Its author first attempted indexed catalog access and then
redesigned the traversal around `filter`, `map`, and `fold`. The redesign was
valid, but the language should support both value-oriented traversal and
position-oriented algorithms without requiring indexing syntax.

These operations are small enough to specify completely. They preserve the
existing immutable Array model and reuse its generic native-module interface,
quota account, heap ownership, and source-location rules.

## Goals

1. provide total zero-based Array access without exceptions or sentinel
   values;
2. provide stable ordered index/value pairs without a callback protocol;
3. preserve the source element type through generic signatures;
4. preserve existing element provenance while locating new wrappers at the
   authored call;
5. use existing fuel, allocation, module, and tool-stage behavior; and
6. keep implementation and diagnostics inside the existing core Array
   boundary.

## API

The declarative native module adds:

```telora
native get: for(A) Fn(Array(A), Int) -> Option(A);
native enumerate: for(A) Fn(Array(A)) -> Array(Tuple([Int, A]));
```

Both names are exported from `std/array` and are ordinary module members. They
are not prelude bindings and do not add member or subscript syntax.

### `get`

```telora
array.get(values, index)
```

Indices are zero-based `Int` values.

- When `0 <= index < array.length(values)`, the result is `'Some(value)` for
  the source value at that index.
- A negative index returns `'None`.
- An index greater than or equal to the Array length returns `'None`.
- Empty Arrays therefore return `'None` for every index.

Negative indices do not count from the end. Returning `Option(A)` makes absence
explicit and keeps access total; there is no Array bounds exception.

### `enumerate`

```telora
array.enumerate(values)
```

For an input `[v0, v1, ..., vn]`, the result is:

```telora
[(0, v0), (1, v1), ..., (n, vn)]
```

The result length equals the source length. Source order and duplicates are
preserved. The empty Array produces `[]`.

Every index must fit Telora's `Int` representation. Runtime Arrays are already
bounded by host and allocation limits, but an unrepresentable position is
nevertheless an `IntegerOverflow` runtime error rather than a wrapped index.

## Static semantics

The signatures preserve the exact element descriptor:

```text
Array<A> x Int -> Option<A>
Array<A>       -> Array<Tuple<[Int, A]>>
```

This includes Struct, Enum, Tuple, Function, metadata, and generic family
instances. Empty input may obtain `A` from an explicit Array type or the
surrounding expected result, following existing generic inference rules.

The first argument must be an Array and `get`'s second argument must be an
`Int`. Static mismatches use the ordinary native contract diagnostics. The VM
retains runtime boundary checks for invalid bytecode or erased dynamic input.

## Runtime representation

`get` reads the source Array through the existing layered heap view. A present
result allocates the ordinary tagged `'Some(payload)` object. An absent result
uses the canonical built-in `'None` Atom and allocates no compound value.

`enumerate` allocates one two-slot Tuple per source element and one outer Array.
Tuple payload values are the newly created `Int` and the original rich source
value. No source element is copied through the legacy value boundary and no
mutable builder is observable by Telora code.

Both operations are synchronous `CoreArrayFunction` variants. Neither invokes
user code or creates a native continuation.

## Provenance

`get` retains the selected element's existing rich-value location as the
`'Some` payload. The new Option wrapper uses the authored core call location.
`'None` uses the call location.

`enumerate` retains each source element's existing location. New index values,
Tuple wrappers, and the outer Array use the authored core call location. This
matches the distinction used by existing Array operations: collection
structure is produced by the rule, while retained data values keep their
source provenance.

## Fuel and allocation

Each operation consumes the ordinary one call unit charged by native dispatch.
Neither adds per-element traversal fuel, consistent with RFC 0010 and RFC 0015.

Allocation is charged before heap mutation:

- successful `get` charges two logical value slots for the tagged result;
- absent `get` charges zero output slots; and
- `enumerate` of length `n` charges `3 * n` logical value slots: two for each
  Tuple and one for each outer Array element.

Length multiplication and `usize`/`Int` conversions are checked. Overflow or
insufficient quota fails before the result graph is allocated. There is no
double charge for installing already charged output objects.

Tool-stage calls use the module initialization account; runtime calls use the
session account. No separate quota or cache is introduced.

## Diagnostics

Errors retain the ordinary `std/array.get` or `std/array.enumerate` frame and
the authored call location. Relevant failures are:

- non-Array first argument at a dynamic boundary;
- non-Int `get` index at a dynamic boundary;
- index conversion or enumeration index overflow;
- logical output-size overflow; and
- allocation quota exhaustion.

Bounds absence is a value result, not a diagnostic.

## Compatibility

This is an additive module change. Existing imports, function identities,
Array runtime layout, callback continuation behavior, and type syntax are
unchanged. Open imports may observe two new names, as expected for additions to
the versioned standard module.

## Implementation plan

1. Add `Get` and `Enumerate` to `CoreArrayFunction`, including stable names and
   arities.
2. Register both functions and expose their generic contracts from
   `array.native.telora`.
3. Implement both synchronous paths in the existing Array dispatcher with
   checked conversions, rich locations, and exact allocation charges.
4. Extend unreachable continuation matches to classify both operations as
   synchronous.
5. Add static, runtime, empty/bounds, ordering, provenance, quota, tool-stage,
   and dynamic-boundary tests.
6. Update this RFC to `Implemented` with the observed result.

## Acceptance criteria

1. `get(["a", "b"], 0)` is `'Some("a")` and index `1` selects `"b"`.
2. Negative, equal-to-length, greater-than-length, and empty access return
   `'None` without a bounds diagnostic.
3. `enumerate(["a", "b"])` is `[(0, "a"), (1, "b")]`; empty input is `[]`.
4. Static results retain `A` exactly and use canonical
   `Tuple([Int, A])` metadata.
5. Source elements preserve provenance; new indices and wrappers point to the
   core call.
6. Present `get` and `enumerate` enforce the exact documented output allocation
   boundary; absent `get` performs no output allocation.
7. Calls work identically at tool stage and runtime stage.
8. Dynamic type failures, integer overflow, allocation failure, and core trace
   names remain sourced and deterministic.
9. Existing Array and workspace tests remain green under strict Clippy.

## Implementation result

`std/array` now exports the two accepted generic contracts through its
declarative native module. `CoreArrayFunction::Get` and `Enumerate` run as
synchronous core calls through the existing layered heap view; neither enters
the callback-continuation path.

`get` converts non-negative indices with a checked host conversion, returns the
canonical `'None` for every absent position, and allocates the two-slot tagged
object only for `'Some`. `enumerate` checks that its length fits `Int`, checks
the `3 * n` logical output size, charges the complete output before mutation,
and then constructs call-located indices and Tuple/Array wrappers around the
original rich source values.

Tests cover empty and boundary access, order, static generic results, tool-stage
metadata construction, dynamic boundary errors, exact fuel/allocation limits,
JSON element provenance, generated-index call provenance, and the existing
Array operation suite.

## Non-goals and deferred work

- `find_index`, `position`, `distinct`, or `distinct_by`;
- slicing, ranges, insertion, replacement, or removal;
- negative-from-end indexing;
- `array[index]` or method syntax;
- lazy iterators, generators, streams, or a general iterable protocol;
- indexed variants of `map`, `filter`, or `fold`; and
- changing Array storage, immutability, or asymptotic access guarantees.

## Rejected alternatives

### Add subscript syntax first

Syntax would need separate decisions for absence, bounds diagnostics, negative
indices, assignment, and pattern use. An ordinary total function establishes
the useful semantics without expanding the language grammar.

### Define negative indices from the end

That convention is convenient but makes an otherwise direct Int-to-position
mapping contextual and differs across languages. This RFC keeps all negative
indices absent; a future slicing proposal may evaluate end-relative positions
as one coherent design.

### Implement `enumerate` as `map` with an indexed callback

Existing `map` intentionally has a one-argument callback. Changing its arity or
adding callback overloading would affect inference, callback validation, and
continuation semantics. A direct operation is simpler and allocates only its
documented result.

### Return a Struct instead of a Tuple

The pair has positional, universally understood semantics and composes with
`Tuple([Int, A])`, `zip`, destructuring, and metadata constructors. Introducing
a named record would add vocabulary without adding information.
