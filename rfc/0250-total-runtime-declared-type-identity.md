# RFC 0250: Total Runtime Declared-Type Identity

- Status: Accepted
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
is a symbolic analysis template. It remains in `TypeDescriptor`/`TypeExpr` and
must fail at the family-application or metadata-construction boundary if code
attempts to materialize it without concrete arguments. No placeholder
`TypeId`, `Any`, name-based identity, or delayed runtime fallback is allowed.

Concrete Type equality compares `TypeId` directly. Formatting obtains the
canonical name through the same identity. Copy collection canonicalizes the
possibly substituted constructor key before allocating the target metadata;
failure aborts the copy with its original boundary context.

## Implementation

1. Replace `Object::DeclaredType.type_id: Option<TypeId>` with `TypeId`.
2. Make declared metadata constructors and family application return errors
   when canonicalization fails.
3. Make copy-collector identity relocation return `Result<TypeId, HeapError>`
   and remove all `.ok()` suppression.
4. Register every allocated declared metadata object by its mandatory ID.
5. Remove equality and formatter branches for missing runtime identity.
6. Keep symbolic templates solely in the analysis/type-family representation.

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

