# RFC 0250: Total Runtime Declared-Type Identity

- Status: Implemented
- Tracking issue: #97
- Depends on: RFC 0237, RFC 0238, RFC 0248, RFC 0249

## Summary

Every declared Type value stored in a runtime Heap will carry a canonical
`TypeId`. Symbolic type-family templates remain analysis data and cannot be
materialized as ordinary runtime Type values until their arguments are
concrete. Recursive declared types reserve their canonical identity before
their descriptor body is sealed.

## Motivation

`Object::DeclaredType` currently stores `Option<TypeId>`. Constructors and the
copy collector turn canonicalization failures into `None`, allowing a sealed
runtime object whose identity is missing. Equality then fails later with an
unfrozen-metadata error, while formatting and copying use different fallback
behavior. This makes an analysis-state leak observable at unrelated runtime
operations and breaks reflexive equality for an otherwise valid Type value.

## Semantics

Runtime declared-Type metadata has two independent properties:

- `type_id: TypeId` is allocated when the metadata object is reserved and is
  always present; and
- `sealed: bool` states whether the recursive descriptor body has been filled.

`TypeStore::begin(TypeConstructorId, TypeArgs)` allocates or finds the stable
identity before recursive body construction. `seal` completes the descriptor;
it does not assign identity.

A descriptor containing an unresolved `Named`, `Bound`, or inference variable
is a symbolic analysis template. Tool-stage evaluation may encode it as the
separate `SymbolicType` WorkWorld value so ordinary Telora family functions can
compose metadata. It is never a `DeclaredType`, cannot own a runtime value, and
has no `TypeId`. No placeholder `TypeId`, `Any`, name-based identity, or
delayed runtime fallback is allowed.

Polymorphic function bodies remain runtime-erased. A literal whose inferred
owner is `Family(Bound(T))` therefore does not capture symbolic owner metadata.
When a call expression has a concrete declared result, the compiler installs
the canonical owner for that concrete call result. This preserves shared
generic code while ensuring `Family(String)` and `Family(Int)` receive their
distinct canonical witnesses rather than both becoming `Family(Any)`.

Concrete Type equality compares `TypeId` directly. Formatting obtains the
canonical name through the same identity. Copy collection canonicalizes the
possibly substituted constructor key before allocating the target metadata;
failure aborts the copy with its original boundary context.

## Implementation

1. Replace `Object::DeclaredType.type_id: Option<TypeId>` with `TypeId`.
2. Make declared metadata constructors and family application return errors
   when canonicalization fails.
3. Represent tool-stage symbolic applications as `SymbolicType`; materialize
   them as concrete `DeclaredType` objects when family substitution closes all
   arguments.
4. Make copy-collector identity relocation return `Result<TypeId, HeapError>`
   and remove all `.ok()` suppression.
5. Register every allocated declared metadata object by its mandatory ID.
6. Remove equality and formatter branches for missing runtime identity.
7. Emit concrete declared ownership at concrete call-result boundaries, never
   for a `Bound` owner inside an erased polymorphic body.

## Rejected alternatives

- `Option<TypeId>` plus an equality error retains contradictory runtime state.
- `Any` or a reserved fallback ID merges distinct family applications.
- Comparing names or descriptor structure at runtime bypasses canonical nominal
  identity and reintroduces recursive deep comparison.

## Acceptance criteria

1. No runtime `DeclaredType` contains `Option<TypeId>`.
2. No declared-type canonicalization error is discarded with `.ok()`.
3. Every decoded runtime declared Type immediately yields a canonical ID.
4. Concrete Type equality deterministically compares IDs.
5. Recursive struct/enum, parameterized families, family composition,
   decorators, same-module references, and cross-World copies pass.
6. Equal `(TypeConstructorId, TypeArgs)` applications intern to one `TypeId`;
   unequal applications remain distinct.
7. Symbolic metadata materialization reports at its construction boundary.
8. Workspace tests and recursive performance fixtures do not regress.

## Outcome

`Object::DeclaredType` now stores a mandatory canonical `TypeId`. Constructors,
family applications, and copy collection propagate canonicalization errors;
runtime equality and formatting no longer contain an unfrozen-identity branch.
Heap errors can retain an owned canonicalization message rather than replacing
it with a later generic failure.

Tool-stage `Family(Bound(T))` values use the distinct `SymbolicType` Heap kind.
Family substitution converts that kind to `DeclaredType` only after all
identity arguments become concrete. Symbolic owners are omitted from compiled
polymorphic bodies; concrete declared call results install their canonical
witness at the call site. The regression constructs `Box(String)` and
`Box(Int)` through one shared generic function and verifies distinct arguments
without `Any` erasure.

Verification on the implementation branch:

- `cargo test --workspace`: 20 LSP, 36 CLI, and 540 core tests passed; one
  pre-existing manual parser baseline remains ignored;
- `cargo check --workspace`: passed with the two existing unused-item warnings;
- source audit found no declared canonicalization `.ok()`, optional runtime
  `DeclaredType` ID, or unfrozen equality fallback;
- `cargo fmt --all -- --check` differs only at the pre-existing untouched
  `module.rs` formatting site; and
- the RFC 0242 release protocol passed all seven fixtures. Median user times
  were 0.069807s flat functions, 0.113835s recursive functions, 0.162579s
  nested functions, 0.039443s shallow recursive values, 0.036975s growing
  recursive values, 0.304188s QueryBuilder check, and 0.303338s QueryBuilder
  show. None regressed against RFC 0249's accepted values.
