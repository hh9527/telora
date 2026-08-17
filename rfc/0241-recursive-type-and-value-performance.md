# RFC 0241: Recursive Type and Value Performance

- Status: Implemented
- Tracking issue: #87
- Depends on: RFC 0034, RFC 0035, RFC 0235, RFC 0237, RFC 0238

## Summary

Telora will make recursive declared types and shared recursive values cost
proportional to the distinct graph actually inspected. Composite runtime values
remain exclusively heap objects; internal stages retain cheap heap references
instead of projecting them into owned `Value` trees. A declared identity is the
primary comparison and validation key. Structural bodies are traversed only at
boundaries that establish identity or when anonymous structural types are
compared.

This is an umbrella RFC. RFC 0242 establishes reproducible evidence and
regression coverage, RFC 0243 defines declared-identity fast paths, and RFC 0244
defines completed graph memoization. The phase does not change assignability,
Fail propagation, declaration identity, or the authoritative TypeMetadata
model.

## Problem

Issue #83 contains two independent reproductions tracked to completion by #87:

- repeated checks involving nested or recursive descriptors make a realistic
  QueryBuilder take several seconds; and
- a twenty-level value DAG built as `[previous, previous]` takes tens of
  seconds even though it contains only linearly many distinct heap objects.

RFCs 0235 through 0240 made declared Struct and Enum roots nominal, but some
consumers still behave structurally after identity is known. In particular,
declared-value validation revalidates the complete payload, descriptor
normalization can clone a declared body repeatedly. More importantly, the tool
stage stores bindings as legacy owned `Value` trees, creates a new VM for each
expression, imports those trees into a new Heap, and exports the result again.
This repeatedly unfolds a shared Heap graph.

The Work/Main and Work/Work heap copier is not part of this defect: it installs
a forwarding handle before visiting children and already preserves graph
sharing. Object references are copied and compared as cheap `(storage, slot)`
handles. Only interned `String`/`Atom` references require cross-World value
lookup or comparison; their relocation remains governed by the text table.

## Invariants

The phase preserves these rules:

1. equal `DeclaredTypeId` values denote the same declared type;
2. equal structure never manufactures declared identity;
3. raw, decoded, Host-provided, or otherwise unbranded values are structurally
   validated before a declared wrapper is minted;
4. a value already carrying an unforgeable declared owner may use that owner as
   proof of its payload contract;
5. anonymous Struct, Enum, Tuple, Array, Dict, Union, and Function types remain
   structurally checked;
6. internal evaluation does not cross the legacy owned `Value` boundary;
7. active cycles at an unavoidable legacy acyclic `Value` boundary remain
   errors; and
8. memoization may preserve DAG sharing but may not convert an unsupported
   cycle into a successful export.

## Runtime value representation

The target runtime `Val` is a fixed-width copy value, conceptually:

```rust
struct Val {
    loc: Loc,       // source, start, end
    payload: u64,   // immediate bits or a Heap slot
    tag: u32,       // primary kind plus a canonical type witness
}
```

Four tag bits distinguish the primary runtime kind. The remaining bits may
carry a canonical type witness. A raw value has no witness; narrowing validates
its structure once and returns the same payload and provenance with the witness
filled. Narrowing must not allocate an owned wrapper or introduce Rust shared
ownership. Declared identity checks are therefore witness-ID comparisons.

Compiler-side `TypeDescriptor` trees are not runtime values. They are migrated
toward `TypeGraph`/`TypeId` separately; they never justify projecting a runtime
Heap graph into owned `Value`.

## Child RFCs

### RFC 0242: evidence and regressions

The existing issue fixtures become a stable, repeatable performance suite. The
suite records medians rather than a single wall-clock sample, separates the
six known shapes, and adds correctness assertions that prevent an optimization
from skipping required raw-value validation.

### RFC 0243: declared-identity fast paths

Runtime validation accepts a matching trusted declared wrapper without
descending into its payload. Type inference and comparison avoid resolving or
formatting a declared body when owner identity or family arguments are
sufficient.

### RFC 0244: Heap-native tool evaluation

Tool evaluation owns one Main Heap and keeps every binding as a
`PersistentValue`. Expressions load those roots through external constant links,
run in fresh Work Worlds, and publish their result back into the Main Heap.
Legacy projection occurs only at a true Host/API boundary. Completed
memoization is limited to unavoidable owned projections and structural type
comparisons still shown hot by RFC 0242 evidence.

## Stopping rules

Each child RFC must land with correctness tests and before/after measurements.
An optimization is rejected if it:

- trusts a raw value merely because an expected declared type is available;
- keys correctness on display text, `Debug` output, or heap addresses that do
  not have stable identity semantics;
- suppresses a mismatch that strict execution currently reports;
- changes the successful exported value; or
- hides exponential work by lowering a fixture depth.

## Acceptance criteria

This phase is complete when:

1. all issue #83 fixtures succeed under their existing semantics;
2. the growing recursive-value fixture scales with distinct graph nodes rather
   than its unfolded tree;
3. realistic QueryBuilder `check` and no-match `show` no longer spend seconds
   repeatedly expanding declared descriptors;
4. raw values still require full structural validation before branding;
5. mismatched declared identities remain deterministic errors;
6. cyclic legacy export remains rejected while repeated DAG nodes are reused;
7. tests cover type comparison, validation, copy, publish, and projection
   boundaries; and
8. workspace formatting, linting, and tests pass.

## Outcome

The accepted release build retains tool-stage values as Heap roots, reuses
completed nodes at unavoidable legacy projections, and shares immutable
compiler-side declared descriptor bodies. The latter are analysis data, not VM
values and not a cross-World representation.

RFC 0242's three-sample median user times changed as follows on the same host:

| fixture | baseline | accepted |
|---|---:|---:|
| flat-functions | 0.194514s | 0.076314s |
| recursive-functions | 0.759197s | 0.116674s |
| nested-functions | 1.863950s | 0.217471s |
| recursive-values-shallow | 0.098112s | 0.040793s |
| recursive-values-growing | 16.166534s | 0.038608s |
| query-builder-check | 7.241579s | 0.405207s |
| query-builder-show | 3.640251s | 0.401056s |

The growing DAG is approximately 419 times faster and no longer depends on its
unfolded leaf count. QueryBuilder `check` and `show` are both below half a
second. Besides graph representation changes, `check` now reuses recovery's
strict finalization result instead of executing the complete module twice.
