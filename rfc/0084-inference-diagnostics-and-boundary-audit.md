# RFC 0084: Inference diagnostics and boundary audit

- Status: Implemented
- Depends on: RFC 0070 through RFC 0083
- Tracking issue: https://github.com/hh9527/forma/issues/1

## Summary

Forma gives rank-1 inference failures a stable conceptual vocabulary and
audits every publication boundary used by the CLI, LSP, module interfaces, and
workspace snapshots. Diagnostics distinguish missing evidence from conflicting
evidence, rigid explicit requirements, non-generalizable obligations,
monomorphic recursion, and unresolved `_` type arguments.

Successful analysis publishes only completed descriptors or declared schemes.
Solver identities, numeric-domain markers, and placeholders remain internal.
Cancellation, recovery, and stale revisions cannot publish a provisional
substitution or scheme.

This RFC stabilizes failure categories and boundary invariants. It does not
introduce a new diagnostic object model or change accepted source programs.

## Diagnostic categories

### Missing evidence

An ordinary generic instance with no concrete solution reports:

```text
cannot infer generic result type Array<?A>
```

The diagnostic is owned by the call expression. It means all available
constraints were compatible but insufficient. It must not claim a conflict or
silently replace the obligation with `Any`.

### Placeholder obligation

An unresolved RFC 0081 source placeholder reports:

```text
cannot infer type argument `_` for parameter "A"
```

Its primary location is the `_` token. This category takes precedence over the
generic-result message because the user explicitly marked the position whose
inference failed.

### Conflicting evidence

Incompatible concrete evidence reports both descriptors:

```text
cannot unify String with Int
```

Argument order may determine which descriptor is presented first, but the
classification and source location remain deterministic. A conflict is never
reported as missing evidence.

### Rigid explicit requirement

An explicit metadata argument or declared `for(...)` parameter is rigid.
Conflicting values use the ordinary incompatibility diagnostic, while invalid
metadata and type-argument arity retain their dedicated messages. The checker
does not weaken a rigid requirement into an inference variable.

### Non-generalizable obligation

A binding whose owned variables cannot be generalized reports a binding-level
completion failure. Numeric-domain variables are the principal current case:

```text
cannot infer monomorphic binding "negate": unresolved Fn(?A) -> ?A
```

RFCs may refine this wording later, but the failure must remain distinct from
an unconstrained generic call and must identify the binding boundary.

### Recursive monomorphic component

An uncontracted recursive component remains monomorphic. Conflicts and missing
evidence are reported at the responsible definition or component, never by
publishing an inferred scheme. An explicit contract is the escape hatch for a
public recursive API, not implicit polymorphic recursion.

## Primary locations

Primary locations follow ownership:

| Failure | Primary source |
| --- | --- |
| unresolved `_` | placeholder token |
| generic result lacks evidence | call expression |
| argument or expected-result conflict | conflicting expression |
| binding cannot complete/generalize | binding name or initializer |
| recursive component cannot complete | responsible definition initializer |
| invalid explicit metadata | metadata argument expression |

Existing declaration, rule, and external-data secondary labels remain intact.
RFC 0084 does not require multi-label reconstruction for every unification
path.

## Publication matrix

The complete analyzer publishes these views:

| Surface | Definition | Use/expression |
| --- | --- | --- |
| `Analysis` | scheme plus erased runtime shape | monomorphic descriptor |
| module interface | exported `TypeScheme` | not applicable |
| CLI `types` / `show` | scheme when available | monomorphic descriptor |
| LSP hover | scheme at definition | monomorphic descriptor at reference |
| workspace snapshot | completed fact | completed or explicitly unavailable fact |

`Bound` identities occur only inside the `TypeScheme` that declares them.
`InferenceVariableId`, placeholder obligations, and numeric-domain membership
occur in none of these surfaces. A runtime function shape may erase static
parameters to `Any`, but it never replaces the separate scheme fact.

## Completion audit

Before a complete `Analysis` is constructed, the checker verifies:

1. every delayed monomorphic binding has completed;
2. every placeholder obligation has completed;
3. every published scheme binds all `Bound` identities in its body;
4. no scheme body contains an inference descriptor;
5. module exports reference only completed schemes; and
6. result and binding descriptors contain no unresolved solver identity unless
   that identity has been deliberately represented by an erased runtime shape
   paired with its authoritative scheme.

Expression records are resolved after all substitutions. Expressions inside a
generic definition may use the existing erased monomorphic presentation when
there is no standalone scheme context for that subexpression; they never expose
the solver's numeric ID or masquerade as a new scheme.

## Recovery, cancellation, and stale revisions

Complete and recoverable analysis remain distinct. Recovery may publish
`Unknown`, `Conflicted`, or `Incomputable` semantic fact states. It must not
manufacture a concrete type or generic scheme from incomplete inference.

Query cancellation checks remain inside inference, unification, recursive
equation solving, component traversal, and module publication. A cancelled or
stale build does not replace the last complete snapshot. Partial substitutions
are owned by the abandoned analysis instance and are not shared.

## Determinism

For identical source and inputs, complete analysis produces stable:

- scheme parameter order and presentation names;
- monomorphic descriptors;
- primary diagnostic category, message, and location;
- module interface contents; and
- CLI/LSP fact selection.

Hash iteration, cache state, scheduling, and cancellation timing do not affect
these results. Recovery diagnostics may grow as more independent work
completes, but an unchanged complete region retains the same fact identity.

## Runtime behavior

This RFC has no runtime behavior. Diagnostics and semantic facts do not add
type arguments, specialization, metadata dictionaries, bytecode instructions,
or VM state.

## Goals

1. distinguish missing evidence, conflict, rigidity, non-generalization,
   recursion, and placeholder failures;
2. assign stable primary source ownership to each category;
3. preserve scheme facts separately from erased runtime shapes;
4. prevent internal solver identities from reaching published surfaces;
5. keep complete and recovery fact states distinct;
6. preserve atomic cancellation and stale-revision behavior;
7. align CLI and LSP definition/use presentation;
8. audit every binding class from RFC 0080; and
9. leave runtime behavior unchanged.

## Non-goals

- a general constraint-explanation engine;
- full provenance for every unification edge;
- changing the LSP protocol or diagnostic wire format;
- warnings or lint levels;
- accepting new inference programs;
- higher-rank, traits, constrained generics, or polymorphic recursion.

## Implementation plan

1. centralize completion and publishability checks for descriptors and schemes;
2. preserve the dedicated placeholder diagnostic priority;
3. audit call, binding, explicit application, and recursive diagnostics;
4. verify definition schemes and use instances in `Analysis` and interfaces;
5. add CLI/LSP hover, recovery, cancellation, conflict, placeholder, numeric,
   alias, recursive, and cross-module boundary regressions;
6. document any intentional erased presentation separately from scheme facts;
7. run full workspace tests and strict static checks.

## Acceptance criteria

1. missing and conflicting evidence have distinct stable messages;
2. unresolved `_` points to the placeholder and wins diagnostic priority;
3. explicit arguments remain rigid and invalid metadata remains dedicated;
4. numeric obligations cannot become unconstrained scheme parameters;
5. recursive components publish no implicit scheme;
6. aliases remain one monomorphic instance;
7. definitions show schemes while references show instances;
8. imports preserve schemes without publishing unresolved identities;
9. recovery uses unavailable fact states instead of guessed types;
10. cancellation and stale builds retain the previous complete snapshot;
11. no runtime representation or behavior changes; and
12. workspace tests and strict static checks pass.

## Rejected alternatives

### One generic `type inference failed` message

It hides whether the user should add evidence, remove a conflict, specify a
type argument, annotate recursion, or choose an explicit dynamic boundary.

### Expose inference IDs in stable tooling

IDs are allocation details and vary with traversal. Stable tools show declared
parameter names, completed descriptors, or an unavailable state.

### Publish provisional schemes during recovery

A plausible but wrong scheme is more damaging than an explicit unavailable
fact because completion, navigation, and downstream module checking would rely
on it.

## Implementation result

Implemented as an explicit scheme-publication audit in the complete analyzer.
Inferred scheme bodies are first normalized through the final substitution
state. Every top-level definition scheme and module export is then rejected if
it contains an inference descriptor or references a `Bound` identity not
declared by that scheme.

The implementation preserves one necessary distinction found during the
audit. A module result may contain an erased runtime function shape whose
generic positions are represented as `Any`; its authoritative `TypeScheme` is
published separately in the module interface. Rejecting the unresolved
pre-erasure result descriptor would incorrectly reject every generic standard
library module. Nested schemes may likewise reference an outer rigid `Bound`;
self-containment is required at the top-level publication boundary, not inside
an enclosing generic checking context.

A corrective import audit applies the same distinction to imported bindings.
The legacy shallow precheck receives an erased descriptor, while the complete
inference engine retains and freshly instantiates the authoritative imported
scheme. Raw `Bound` identities from one scheme therefore cannot be compared by
private numeric ID with an unrelated enclosing scheme. This removes a
shape-sensitive selective-import rejection without changing intentional
`Any` boundaries for Host or dynamic values that have no interface scheme.

Existing dedicated diagnostics remain authoritative: placeholder errors point
to `_`, generic calls distinguish missing evidence from unification conflicts,
numeric obligations cannot generalize, and recursive components publish no
implicit scheme. Regression coverage now directly rejects solver identities
and unbound parameters in publishable schemes, while the existing CLI, LSP,
recovery, cancellation, alias, recursion, and module-interface suites exercise
the complete boundary matrix.
