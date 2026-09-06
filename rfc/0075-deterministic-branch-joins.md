# RFC 0075: Deterministic branch joins

- Status: Implemented
- Partial supersession: RFC 0268 removes the public untagged Union fallback;
  incompatible branches require a common type context. Order independence remains.
- Depends on: RFC 0052, RFC 0070, RFC 0071, RFC 0073, RFC 0074

## Summary

Forma gives `if` and `match` one pure, deterministic branch-join operation.
Joining result types never mutates inference substitutions and never chooses a
narrower branch merely because it was visited first.

```forma
if condition { Int } else { String } # Type
if condition { 1 } else { "text" }   # Int | String
```

When no common existing supertype is known, the result is a canonical Union:
nested Unions are flattened, `Never` is removed when a reachable alternative
exists, duplicates are removed, and members use stable type order.

## Motivation

The current checker speculatively unifies fully resolved branch results. This
uses an equality solver to answer a least-upper-bound question. Because
unification accepts existing assignability relationships, two metadata
witnesses may incorrectly retain the first witness instead of joining to
`Type`. The same strategy snapshots and restores substitutions for `match`,
which is difficult to reason about and makes arm order observable.

Branch checking and branch joining are separate operations:

```text
check each branch <= surrounding expected type
join inferred branch result types without adding constraints
```

## Goals

1. define one join operation shared by `if` and `match`;
2. make joining pure with respect to inference substitutions and domains;
3. preserve RFC 0070's `Never` rules;
4. retain identical types without constructing a Union;
5. select an existing unique wider type using directional assignability;
6. join distinct TypeMetadata witnesses as `Type`;
7. flatten nested Unions recursively;
8. remove duplicate members and sort them deterministically;
9. normalize joins again after delayed variables resolve;
10. keep expected-type checking authoritative for every branch.

## Non-goals

- subtyping beyond Forma's existing assignability relation;
- collection covariance or lifting `Array(A) | Array(B)` to `Array(A | B)`;
- row polymorphism or structural width subtyping;
- flow-sensitive narrowing or exhaustiveness;
- solving inference variables from sibling branch results;
- changing runtime values or branch execution.

## Join rules

For resolved descriptors, `join(a, b)` applies these rules in order:

```text
join(Never, T) = T
join(T, Never) = T
join(T, T) = T
join(TypeOf(A), TypeOf(B)) = Type, when A != B
join(A, B) = B, when A <= B and not B <= A
join(A, B) = A, when B <= A and not A <= B
join(Any, T) = Any
join(A, B) = canonical_union(A, B), otherwise
```

The directional cases select a unique existing wider descriptor. If both
directions succeed only because of a dynamic boundary, `Any` wins explicitly;
the implementation must not use arm order as a tie-breaker.

An unresolved inference variable is retained as a Union member. Join does not
bind it to its sibling. Delayed resolution later canonicalizes the enclosing
Union. If that variable remains unresolved, its owning binding fails under RFC
0073.

## Canonical Unions

Canonicalization recursively flattens Union members, resolves available
inference substitutions, removes `Never` when another member exists, removes
structural duplicates, and sorts by a stable descriptor key. A Union containing
`Any` normalizes to `Any`.

No singleton or empty Union is published. A singleton becomes its member; an
all-`Never` input becomes `Never`.

## Expected mode

With a surrounding expected type, every branch is checked against it before
joining:

```forma
let value: Int = if condition { 1 } else { stop() };
```

The expression records the precise joined result `Int`. A failing branch is
reported at that branch, not hidden by a wider sibling or by the final join.

Join itself never supplies evidence to external inference variables. Evidence
comes only from directional branch checks against the surrounding expectation.

## Diagnostics and facts

Join introduces no new failure mode. Incompatibilities arise while checking an
expected type or later consuming the resulting Union. Stable canonical order
keeps diagnostics, CLI output, hover, and module interfaces independent of arm
order.

## Implementation plan

1. replace speculative `try_unify` branch logic with a pure join helper;
2. share the helper between `if` and `match`;
3. recursively canonicalize Union descriptors during inference resolution;
4. define stable ordering for canonical members;
5. preserve TypeMetadata widening and `Never` absorption;
6. add reversed-arm, metadata, nested Union, duplicate, `Any`, `Never`, delayed
   variable, expected-type, semantic-fact, and cancellation tests;
7. run full workspace tests and strict static checks.

## Acceptance criteria

1. reversing `if` branches does not change the displayed result type;
2. reversing `match` arms does not change the displayed result type;
3. distinct TypeMetadata witnesses join to `Type`;
4. heterogeneous values join to a stable Union;
5. nested Unions flatten and duplicates disappear;
6. reachable alternatives absorb `Never`, while all-`Never` remains `Never`;
7. `Any` dominates a join explicitly and symmetrically;
8. join never adds or rolls back substitutions;
9. unresolved delayed variables are retained and later normalized;
10. surrounding expected types still check every branch;
11. no singleton, nested, duplicate, or unstable Union reaches final facts;
12. workspace tests and strict static checks pass.

## Deferred work

- flow-sensitive pattern narrowing and exhaustiveness;
- structural least-upper-bounds requiring variance;
- local closure annotations;
- explicit generic type application;
- recursive SCC inference.

## Implementation result

Implemented in the RFC 0075 change set.

`if` and `match` now use one pure descriptor join. It handles equality,
`Never`, explicit `Any`, TypeMetadata widening, and asymmetric existing
assignability without touching substitutions. All remaining alternatives enter
one canonical Union path.

Union resolution recursively flattens members, lets `Any` dominate, removes
reachable `Never` and structural duplicates, and sorts descriptors by stable
display and structural keys. Delayed variables therefore normalize again after
their substitutions become available, without sibling branches solving them.

Tests cover reversed `if` and `match` order, TypeMetadata witnesses, nested and
duplicate alternatives, delayed closure variables, explicit `Any`, and
`Never`. The final workspace run passed 245 Forma tests with one manual parser
benchmark ignored, 12 CLI tests, and 19 LSP tests; strict Clippy passed.

## Rejected alternatives

### Unify branch results

Unification solves equality obligations and may mutate inference state. A join
asks for a representable common result and must not create evidence merely from
control-flow alternatives.

### Preserve source arm order in Unions

Source order is operationally meaningful for `match`, but it should not change
the type identity or diagnostics of the result set. Canonical type order is a
separate concern from runtime arm order.

### Recursively lift structural constructors

Turning `Array(Int) | Array(String)` into `Array(Int | String)` assumes
collection variance and broadens which concrete arrays the type describes.
That needs a separate subtyping design.
