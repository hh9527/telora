# RFC 0078: Monomorphic recursive closure inference

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Unresolved recursive results require context and are excluded from publication.
- Depends on: RFC 0050, RFC 0052, RFC 0073, RFC 0074, RFC 0075, RFC 0076

## Summary

Unannotated closure-valued `def` bindings may infer one monomorphic function
type through direct or mutual recursion:

```forma
def countdown = fn(value) {
    if value < 1 { 0 } else { countdown(value - 1) }
};
```

infers `Fn(Int) -> Int`. Before checking recursive definitions in a lexical
block, Forma creates one shared function skeleton per eligible definition:

```text
countdown: Fn(?P) -> ?R
```

Bodies, recursive calls, and later non-recursive uses constrain that skeleton.
Every recursive component remains monomorphic and must be fully resolved at
the block boundary.

## Motivation

RFC 0073 deliberately hides an unannotated definition's conservative bootstrap
slot so `Any` cannot masquerade as recursive evidence. This correctly rejects
underconstrained recursion, but it also rejects ordinary functions whose
operators, branches, literals, and calls provide enough concrete evidence.

RFCs 0074 and 0075 now provide deterministic body constraints and branch joins.
Forma can therefore infer useful monomorphic recursion without introducing
polymorphic recursion, implicit schemes, or runtime changes.

## Eligible definitions

The first version considers a binding eligible when it is:

```forma
def name = fn(parameters) { body };
```

and has no `decl` or inline function contract. Parameter and result annotations
from RFC 0076 may be partial and participate as ordinary constraints.

Annotated `def`, `decl + def`, `native`, `let`, imports, and non-closure-valued
definitions retain their existing behavior. A recursive value whose function
shape is not syntactically visible requires an explicit contract.

## Recursive regions and components

Within each lexical block, eligible definitions are visible to every eligible
definition body in that block for type inference, matching the runtime's
single-assignment recursive definition slots.

The checker solves the constraints induced by those definitions as one
deterministic block-local system. An implementation may partition that system
into strongly connected components, but this is an optimization rather than an
observable requirement: definitions without connecting references share no
variables and therefore solve independently either way.

Every recursively connected set shares one substitution state, but no type
variable is generalized or instantiated between calls. Acyclic definitions
continue to use the same skeleton representation and complete under RFC 0073's
ordinary block rules.

## Function skeletons

For a closure with `n` parameters, the checker predeclares:

```text
Fn(?P0, ..., ?Pn) -> ?R
```

Local parameter or result annotations replace the corresponding fresh
positions. A surrounding declared contract remains authoritative and therefore
does not use this inference path.

Checking the closure against its skeleton puts those exact parameter and result
identities into the body environment. A recursive call observes the same
skeleton; it does not instantiate fresh variables.

## Completion

After all bindings and the block result have contributed constraints, every
eligible skeleton is resolved. If a recursive component retains any owned
inference variable, analysis fails:

```text
cannot infer recursive definition `loop`: unresolved Fn(?0) -> ?1;
add a decl or inline contract
```

An explicit `Any` annotation is a concrete dynamic boundary and may complete a
position. An accidental conservative bootstrap `Any` is never evidence.

No unresolved skeleton reaches binding facts, TypeGraph, module interfaces,
hover, or bytecode compilation.

## Examples

Direct recursion:

```forma
def sum_to = fn(value) {
    if value < 1 { 0 } else { value + sum_to(value - 1) }
};
```

Mutual recursion:

```forma
def even = fn(value) {
    if value < 1 { 'True } else { odd(value - 1) }
};
def odd = fn(value) {
    if value < 1 { 'False } else { even(value - 1) }
};
```

Both infer an `Int` parameter and normalized `Bool` result.

Underconstrained recursion remains invalid:

```forma
def loop = fn(value) { loop(value) };
```

The author must write a contract when the body supplies no concrete evidence.

## Diagnostics and cancellation

Concrete conflicts point to the smallest recursive call argument or body
result. Unresolved-component diagnostics use the definition initializer as the
primary location and may list other component members as secondary context.

Dependency scanning, component traversal, body checking, and completion retain
query cancellation checkpoints. Cancellation publishes neither provisional
skeletons nor partial substitutions.

## Goals

1. infer direct monomorphic recursive closures;
2. infer mutually recursive closures in one lexical block;
3. share exact parameter and result identities across recursive calls;
4. combine body, partial annotation, and later-use evidence;
5. solve recursive constraints deterministically regardless of source order;
6. reject underconstrained or conflicting components;
7. prevent bootstrap `Any` from supplying evidence;
8. keep runtime recursive slots and bytecode unchanged;
9. publish only completed facts and interfaces.

## Non-goals

- polymorphic recursion;
- implicit generalization or recursive `TypeScheme` creation;
- inferring callable shapes for arbitrary recursive expressions;
- recursive types or equi-recursive value descriptors;
- cross-module recursive components;
- forward visibility for `let` bindings;
- termination, totality, or effect analysis.

## Implementation plan

1. identify uncontracted closure-valued `def` bindings per block;
2. collect the block-local constraints induced by definition references;
3. allocate one annotated-or-fresh function skeleton per eligible definition;
4. seed the block environment before checking definition bodies;
5. check each closure against its exact skeleton;
6. solve recursive result equations to a least fixed point, then retain RFC
   0073 delayed completion for all other owned variables;
7. remove conservative self slots from recursive evidence;
8. resolve binding and expression facts only after component completion;
9. add direct, mutual, nested, partial annotation, later use, conflict,
   underconstrained, explicit contract, no-generalization, semantic-fact,
   runtime, diagnostic-order, and cancellation tests;
10. run full workspace tests and strict static checks.

## Acceptance criteria

1. countdown-style direct recursion infers `Fn(Int) -> Int`;
2. mutually recursive parity functions infer `Fn(Int) -> Bool`;
3. source order of a mutual component does not change its types;
4. partial closure annotations constrain a recursive skeleton;
5. later non-recursive calls can complete remaining positions;
6. all calls in a component share one monomorphic solution;
7. conflicting recursive uses fail deterministically;
8. evidence-free recursion remains an error;
9. arbitrary non-closure recursive values require explicit contracts;
10. explicit recursive contracts retain existing behavior;
11. no recursive binding is implicitly generalized;
12. nested block components complete before escaping;
13. final facts and module interfaces contain no inference variables;
14. runtime recursion and tail-call behavior are unchanged;
15. cancellation prevents provisional publication;
16. workspace tests and strict static checks pass.

## Deferred work

- polymorphic recursion;
- recursive value and type inference;
- component-wide generic generalization;
- cross-module recursion;
- termination and effect analysis.

## Rejected alternatives

### Reuse the conservative Any bootstrap slot

That makes recursion appear solved without evidence and silently publishes a
dynamic contract. Recursive inference requires shared variables, not erased
placeholders.

### Instantiate a fresh function type at each recursive call

That would make one definition behave polymorphically inside its own body and
allow conflicting calls to avoid one monomorphic solution.

### Require annotations for every recursion forever

Explicit contracts remain the escape hatch, but operator and branch constraints
now make common monomorphic recursion both decidable and useful. Rejecting all
of it would discard evidence the checker already has.

## Implementation result

Implemented in the HIR analyzer, type checker, and compiler. Each lexical block
predeclares uncontracted closure-valued `def` bindings with shared monomorphic
function skeletons before analyzing their bodies. Direct and mutual forward
references therefore observe the same parameter and result variables at both
analysis and runtime compilation stages.

Recursive result constraints are represented as finite join equations and
solved by deterministic least-fixed-point iteration. Recursive references
initially contribute no branch evidence; concrete base branches widen the
approximation until it stabilizes. Evidence-free recursion consequently stays
unresolved instead of collapsing to `Any` or `Never`.

The implementation solves every eligible equation in a lexical block together
rather than materializing an explicit SCC graph. This preserves the specified
semantics because disconnected equations share no inference variables, while
keeping SCC partitioning available as a future performance optimization.

Coverage includes direct, mutual, nested, partially annotated, later-constrained,
conflicting, and non-closure recursion, plus runtime execution. Full workspace
tests and strict Clippy checks pass.
