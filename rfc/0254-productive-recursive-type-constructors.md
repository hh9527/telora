# RFC 0254: Productive Recursive Type Constructors

- Status: Implemented
- Tracking issue: #109
- Supersedes: the recursive-family rejection in RFC 0232
- Depends on: RFC 0238, RFC 0239, RFC 0248, RFC 0250

## Summary

Telora will allow a nominal parameterized type constructor to refer directly
to its own application with exactly the constructor's bound parameters, when
that reference occurs below the constructor's declared `struct` or `enum`
root.

```telora
type Expr(A) = enum {
    'Leaf(A),
    'Call(Array(Expr(A))),
};

type IntExpr = Expr(Int);
```

The family template is one finite cyclic `SymbolicType` graph. Applying it to
concrete arguments substitutes the bound parameters and materializes one
canonical recursive declared type identified by
`(TypeConstructorId, TypeArgs)`. Repeated `Expr(Int)` applications therefore
return the same `TypeId`; `Expr(Int)` and `Expr(String)` remain distinct.

This RFC does not add general recursive type functions. Mutual recursive
families, transformed recursive arguments, mixed concrete/family cycles, and
recursive aliases remain rejected.

## Motivation

Closed recursive nominal types and acyclic nominal type families already have
canonical identities, recursive graph sealing, cross-World copying, codecs,
and schemas. Their artificial boundary prevents common eDSL algebras from
retaining their leaf type:

```telora
type Expr(Attribute) = enum {
    'Attribute(Attribute),
    'Call(Array(Expr(Attribute))),
};
```

The current workaround replaces the leaf with an index into an external
catalog. That weakens the type contract and creates a second consistency rule
between the recursive value and the catalog.

The runtime work completed after RFC 0232 removes the old identity and graph
copy blockers. The remaining problem is narrowly the construction of a finite
symbolic family template.

## Accepted syntax and semantics

The accepted form is a nominal family whose declared initializer root is
`struct` or `enum` and whose recursive applications use the current bound
parameters once, in declaration order:

```telora
type List(A) = enum {
    'Nil,
    'Cons({head: A, tail: List(A)}),
};
```

The recursive application denotes a back-edge to the current symbolic family
root. It does not execute or unfold the family body.

The following remain errors:

```telora
type Loop(A) = Loop(A);                  # non-productive alias
type Grow(A) = struct {next: Grow(Array(A))}; # transformed arguments
type Swap(A, B) = struct {next: Swap(B, A)};  # reordered arguments
type Left(A) = struct {right: Right(A)};
type Right(A) = struct {left: Left(A)};   # mutual families
```

"Productive" has a deliberately syntactic meaning in this RFC: the family
declaration itself has a declared Struct or Enum initializer, and every
accepted recursive edge is below that root. Telora does not attempt positivity,
variance, contractiveness, or arbitrary type-function termination proofs.

Decorators apply to the completed recursive root using the existing declared
type protocol. A decorator cannot observe or publish the unsealed placeholder.

## Template construction

For an accepted family `F(P0, ..., Pn)`, analysis performs these steps:

1. Allocate the stable `TypeConstructorId` from the module skeleton.
2. Build the symbolic identity
   `(TypeConstructorId(F), [Bound(P0), ..., Bound(Pn)])`.
3. Reserve one unsealed `SymbolicType` root with that identity.
4. Bind `F` during its own body evaluation to a restricted self-application
   capability. It accepts only the exact bound arguments and returns the
   reserved root.
5. Evaluate the declared Struct or Enum body once and seal the root with it.
6. Validate and publish the resulting finite symbolic graph and the ordinary
   `for(P...) Fn(TypeOf(P)...) -> TypeOf(F(P...))` scheme.

An error before sealing aborts the family construction. No unsealed root,
partial scheme, `Any` approximation, or recovery-only type enters a module
interface or MainWorld.

This mechanism is local to the currently evaluated declaration. It does not
change ordinary name resolution, generic function inference, structural
unification, or nominal equality.

## Concrete instantiation and identity

Applying the sealed template to concrete arguments uses the existing
substitution copy:

1. Canonicalize every concrete argument to `TypeId`.
2. Call `TypeStore::begin(TypeConstructorId, TypeIds)` before copying the body.
3. Copy the symbolic graph with a forwarding entry from its root to the
   reserved concrete declared root.
4. Replace `Bound(Pi)` with argument metadata and recursively copy the finite
   graph.
5. Seal the reserved `TypeId` and declared metadata root.

If `begin` finds an existing ready identity, the existing canonical type is
returned. If it observes the identity already being built through the current
copy, the forwarding entry closes the cycle rather than starting another
instantiation.

Runtime equality continues to compare nominal `TypeId`; it never deep-compares
the recursive descriptor. WorkWorld to MainWorld and WorkWorld to WorkWorld
copy preserve or relocate the graph through their existing forwarding tables.

## Diagnostics and resource behavior

Rejected recursive families produce a sourced declaration diagnostic before
runtime execution. In particular, `type Loop(A) = Loop(A)` is not evaluated
until fuel exhaustion and cannot overflow the host stack.

Template construction evaluates the body once. Concrete application copies
one finite graph and remains subject to the existing allocation, cancellation,
stack, and call-depth quotas. Canonical memoization prevents repeated equal
applications from rebuilding the graph.

Best-effort analysis may continue independent declarations, but it cannot
publish a scheme or usable value for a failed recursive family.

## Implementation plan

1. Classify strongly connected type components and admit only a one-node
   nominal family with exact-parameter self edges.
2. Add the restricted tool-stage self-application binding and symbolic root
   reserve/seal/abort protocol.
3. Teach family instantiation to forward the symbolic root to the concrete
   reserved root before copying children.
4. Keep family identity keyed exclusively by
   `(TypeConstructorId, TypeArgs)`; never by source or display name.
5. Cover analysis, construction, match, codec/schema, imports, and both
   cross-World copy directions.
6. Update LANGUAGE SSOT and the tutorial with the accepted and rejected
   boundary.

## Rejected alternatives

### Eager unfolding

It cannot produce a finite descriptor and makes identity depend on a depth or
fuel setting.

### Re-execute the family body for every application

Type families remain symbolic type functions evaluated once. Re-execution
could inspect concrete metadata and disagree with the published generic
scheme.

### General recursive type-function normalization

Supporting transformed arguments such as `F(Array(A))` requires termination
and potentially infinite-instantiation reasoning. It is not needed for the
motivating recursive algebra.

### Mutual recursive families

They require a component-wide family environment and atomic multi-root
publication. The direct self-recursive scope provides the required value with
substantially less semantic surface.

### Relax nominal or structural unification

The missing operation is template graph closure. Relaxing inference would
merge unrelated nominal identities and move failures away from their
declarations.

## Acceptance criteria

1. `Expr(A)` with direct, exact-argument recursion analyzes and publishes a
   precise constructor scheme without `Any`.
2. `Expr(Int)` can be constructed, matched, encoded, decoded, and described by
   JSON Schema.
3. Repeated `Expr(Int)` applications have one canonical `TypeId`, while
   `Expr(String)` has a different identity.
4. The family survives direct import, selective import, re-export, Work-to-Main
   publication, and Work-to-Work reducer transfer.
5. `Loop(A)`, transformed/reordered arguments, mutual families, and mixed
   family/concrete cycles fail deterministically with sourced diagnostics.
6. No unsealed `SymbolicType`, free `Bound`, or provisional canonical identity
   is published after failure.
7. Existing concrete recursion, acyclic families, inference, equality,
   codec/schema, resource accounting, and best-effort behavior do not regress.
8. Workspace tests, formatting, and diff checks pass.

## Outcome

The type dependency scheduler now recognizes a one-node nominal family cycle.
Full and partial analysis share one construction path: it reserves a symbolic
declared root keyed by the constructor and Bound arguments, installs a
restricted same-argument self capability, evaluates the Struct/Enum body once,
and seals the finite graph. Changed or reordered arguments fail at the authored
family application.

The ordinary family application copier materializes the sealed symbolic graph
as a concrete declared graph. Its existing forwarding map closes the recursive
edge, while `TypeStore` continues to canonicalize concrete identity from
`(TypeConstructorId, TypeArgs)`. No inference or nominal assignability rule was
relaxed.

Regression coverage includes precise full and partial schemes, equal and
unequal concrete applications, construction and matching, deterministic
argument rejection, import through a re-export facade, codec round trips, and
recursive JSON Schema references. Workspace verification passed with 20 LSP,
37 CLI, and 558 core tests; one existing parser baseline remains ignored.
