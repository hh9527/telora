# RFC 0092: Structural `Dyn` observers

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Observer errors are AccessError with the original Dyn input.
- Depends on: RFC 0089 through RFC 0091
- Tracking issue: https://github.com/hh9527/forma/issues/4

## Summary

`@bim/std/dyn` gains safe, read-only structural observers that advance a
runtime value and its canonical descriptor together:

```forma
native field: Fn(Dyn, String) -> Result(Dyn, BlameError);
native array_items: Fn(Dyn) -> Result(Array(Dyn), BlameError);
native tuple_items: Fn(Dyn) -> Result(Array(Dyn), BlameError);
native tag: Fn(Dyn) -> Result(String, BlameError);
native payload: Fn(Dyn) -> Result(Option(Dyn), BlameError);
```

Every returned child `Dyn` preserves RFC 0091's existential invariant. No API
returns a child `Type` separately from an erased child value.

## Descriptor preparation

Before selecting an operation, an observer resolves `$ref` edges and strips
`WithAttributes` wrappers from its local descriptor view. This normalization
does not mutate the descriptor stored in the input `Dyn`; `dyn.desc` continues
to return the exact canonical node.

Recursive fields may therefore return a child `Dyn` whose descriptor is the
original `$ref`. A caller can observe `'Ref` through RFC 0090 and the next
structural observer resolves it when traversal continues.

## Field observation

`field(value, name)` supports:

- Struct: the name must exist in both the descriptor and runtime Dict;
- Dict(T): the runtime key must exist and every child uses descriptor `T`.

Struct/Record values are not treated as untyped Dicts. A runtime field without
a declared descriptor, a declared field missing at runtime, a wrong runtime
shape, or an unknown requested field returns structured blame.

The child package contains the selected canonical field descriptor and runtime
field value.

## Sequence observation

`array_items` accepts only `Array(T)` plus a runtime Array and packages each
item with the same `T` descriptor.

`tuple_items` accepts only Tuple metadata plus a runtime Tuple. Descriptor and
runtime lengths must agree; each position is packed with its corresponding
descriptor.

The APIs are separate so a caller cannot accidentally interpret a Tuple as a
homogeneous Array.

## Tagged observation

`tag` accepts Atom, Tagged, or Enum descriptors and matching runtime values. It
returns the logical tag name as String.

`payload` returns:

- `'None` for a unit Atom or unit Enum variant;
- `'Some(Dyn)` for Tagged and payload Enum variants; or
- `Err(BlameError)` for descriptor/value disagreement.

For Enum, the runtime tag selects the declared variant before its payload
descriptor is paired with the runtime payload. Unknown variants and unit versus
payload disagreement are errors.

## Blame

Observer failures use `BlameError` with:

- `data`: the input `Dyn`;
- `rule`: a stable observer name such as `"dyn.field"`; and
- `message`: the requested operation, expected descriptor/runtime shape, and
  field or variant name where relevant.

The first API does not add a hidden path stack. An interpreter that recursively
walks values owns its semantic path and may wrap or enrich observer errors.

## Goals

1. preserve descriptor/value correspondence through every child projection;
2. distinguish Struct, Dict, Array, Tuple, Atom, Tagged, and Enum semantics;
3. make recursive data traversal possible with ordinary Forma recursion;
4. turn every public shape mismatch into value-level blame; and
5. avoid operation-specific equality or codec behavior in the observer layer.

## Non-goals

- mutation, construction, clone, decode, or `Dyn -> A` recovery;
- Union branch selection;
- Function inspection;
- automatic recursion, path accumulation, fallback, or memoization;
- cyclic runtime values;
- field-shape inference or structural subtyping; or
- exposing Dict shape, heap identity, or descriptor handles.

## Acceptance criteria

1. Struct field projection returns a child with the declared descriptor;
2. homogeneous Dict field projection reuses its item descriptor;
3. missing and undeclared fields return structured blame;
4. Array items and Tuple positions preserve their descriptors and order;
5. wrong sequence kind and Tuple length disagreement return blame;
6. Atom and unit Enum variants report a tag and no payload;
7. Tagged and payload Enum variants return the correct child `Dyn`;
8. unknown tags and payload-shape disagreement return blame;
9. a finite recursive Struct/Enum value can be traversed to a primitive leaf;
10. Function and unsupported descriptors are rejected without inspection; and
11. quotas, cancellation, copying, and publication remain bounded and atomic.

## Implementation plan

1. extend the `CoreDynFunction` operation family;
2. add local descriptor resolve/attribute normalization helpers;
3. implement one checked child-pack primitive used by every observer;
4. implement Struct/Dict, Array/Tuple, and Atom/Tagged/Enum branches;
5. centralize structured observer blame and allocation accounting;
6. add direct, mismatch, recursive, quota, and publication regressions; and
7. run the full quality gate and record the implementation result.

## Implementation result

Implemented `field`, `array_items`, `tuple_items`, `tag`, and `payload` in the
existing `CoreDynFunction` family. Observation first builds a read-only plan
from one heap view, then allocates all child packages and the Result after
validation succeeds. Failures allocate only a structured `BlameError`; no
partially projected child escapes.

Parent descriptors resolve `$ref` and strip `WithAttributes` locally. Child
packages retain the exact canonical child descriptor, including wrappers and
recursive references. Struct fields are selected from declared metadata and
runtime Dict shape together; Dict items reuse their homogeneous descriptor.
Array and Tuple observers remain distinct and Tuple length is checked.

Atom, Tagged, and Enum observation validates runtime tag, declared variant,
and unit/payload agreement. Tags are returned as String, unit payloads as
`'None`, and payload variants as `'Some(Dyn)`. Function and unsupported kinds
return value-level blame.

Regression coverage includes Struct, Dict, Array, Tuple, payload Enum, unit
Enum, missing fields, wrong shapes, child descriptor narrowing, and a finite
recursive Struct/Array traversal to an Int leaf. Full Forma tests pass with 284
passed and 1 ignored; all 13 CLI integration tests pass.
