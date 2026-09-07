# RFC 0052: Unified bidirectional type checking

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Any assignment, compatibility and inference rules are removed.
- Partial supersession: RFC 0268 removes public untagged Union types and their
  assignment rules. Bidirectional checking and contextual enum construction remain.
- Depends on: RFC 0042, RFC 0048, RFC 0049, RFC 0050, RFC 0051

## Summary

XL uses one authoritative bidirectional checker for ordinary expressions,
annotated definitions, generic calls, and final semantic facts:

```text
environment |- expression => synthesized type
environment |- expression <= expected type
```

Expected types flow through closures, calls, collections, blocks,
conditionals, and matches. Explicit `TypeScheme` bindings instantiate fresh
inference variables at each direct use, while definition bodies are checked
against rigid bound parameters. Local bindings remain monomorphic and the VM
continues to erase all static type machinery.

The existing conservative shape inference may remain as a bootstrap aid while
tool-stage values and contracts are discovered. It is not authoritative for
final binding types, result types, or expression semantic facts.

## Motivation

RFCs 0048 through 0051 added the pieces needed for useful static polymorphism:
data-backed schemes, rigid parameters, fresh use-site variables, annotated
function checking, and the `Type` metatype. Their implementation introduced a
focused generic inference pass beside XL's older conservative expression
inference. Whether an expression receives unification and expected-type flow
currently depends on the presence of schemes or imported interfaces, and some
expressions inside the generic pass still fall back to the older inference.

This split makes ordinary and generic programs observe different checking
rules. It also loses context in important cases:

```xl
def choose: Fn(Bool) -> Array(Int) = fn(condition) {
    if condition { [] } else { [1] }
};

def values: Array(Int) = empty();
```

The empty Array and a generic result are checkable from their expected types.
The checker must propagate that information consistently rather than depend on
an unrelated generic binding elsewhere in the module.

## Goals

1. make bidirectional checking the authoritative expression typing path for
   every complete strict program;
2. run the same checker whether or not the module contains generic schemes;
3. propagate expected types through all expression forms and block results;
4. infer generic calls from arguments, callbacks, and the surrounding expected
   result;
5. check annotated closures with expected parameter and result types;
6. keep direct scheme references freshly instantiated and local aliases
   monomorphic;
7. retain rigid checking of generic definition implementations;
8. record resolved checker results as expression semantic facts;
9. preserve asynchronous cancellation checkpoints during traversal and
   unification;
10. leave TypeMetadata evaluation, runtime values, bytecode, and the VM ABI
    unchanged.

## Non-goals

- implicit generalization or polymorphic local bindings;
- interface, trait, capability, implementation, or associated-type systems;
- bounded, constrained, higher-rank, or higher-kinded polymorphism;
- subtyping, overload resolution, coercions, or specialization;
- inverse solving of arbitrary `Fn(Type) -> Type` metadata functions;
- flow-sensitive narrowing or exhaustive pattern analysis;
- removal of bootstrap shape inference needed before tool-stage contracts have
  been evaluated.

## Authoritative checker

Strict analysis constructs the available monomorphic environment and
`TypeScheme` table while evaluating declarations and TypeMetadata. It then
runs one bidirectional checker over every executable binding and the program
result. This checker runs unconditionally.

The checker owns final:

- inferred binding types;
- program result type;
- resolved expression descriptors;
- incompatibility errors involving ordinary or generic expressions.

Bootstrap inference may provide provisional shapes needed to order and execute
tool-stage work. Those provisional results must not overwrite checker output
or become final semantic facts.

## Synthesis and checking

Literals, known variables, fields, and function calls synthesize types.
Annotations, function parameters, collection elements, branch results, and
call results provide expected types. Checking an expression against an
expected type recursively propagates the expectation and finally unifies the
synthesized result with it.

Expected function types flow into closures:

```xl
def increment: Fn(Int) -> Int = fn(value) { value + 1 };
```

Expected collection and structural types flow into their members. Blocks pass
their expectation to the final expression. `if` and `match` pass the same
expectation to every result branch and unify synthesized branches when no
expectation is available.

A closure without an expected function type retains XL's conservative
parameter behavior: its parameters are `Any`. This RFC does not infer lambda
parameter types from arbitrary use after the closure has been stored.

## Generic calls

Referencing a `TypeScheme` directly replaces each bound parameter with one
fresh inference variable shared throughout that instantiation. Calls constrain
those variables from every argument and from an available expected result:

```xl
native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);
native empty: for(A) Fn() -> Array(A);

def labels: Array(String) = map([1], fn(value) { "item" });
def values: Array(Int) = empty();
```

Inference variables use an occurs check. Any variable still present in a call
result after argument and expected-result constraints have been applied is an
underconstrained-call error. The checker does not guess a type and does not
silently convert that result to `Any`.

`Any` remains an explicit gradual boundary under XL's existing assignability
rules. It is distinct from an unsolved inference variable and from recoverable
syntax or semantic states.

## Bindings and schemes

An annotated `def` or a `decl`/`def` pair checks its implementation against the
declared contract. Generic implementations see rigid `Bound` descriptors;
ordinary uses instantiate fresh inference variables.

An unannotated `let` or `def` synthesizes one monotype. If its right-hand side
directly references a scheme, that reference is instantiated once and the
resolved monotype is stored in the local environment. Later references do not
instantiate it again. This preserves the explicit-generalization policy from
RFC 0049.

Type declarations check their right-hand sides against `Type`. Their represented
instance descriptor remains separately recorded in `declared_types` as defined
by RFC 0051.

## Conditionals and patterns

`if` checks its condition using the existing Bool-compatible policy. Its two
branches receive the surrounding expected type. Without an expectation, their
results are unified or combined under the existing conservative common-type
rules where unification is not appropriate.

`match` first synthesizes the scrutinee. Existing pattern binding inference
extends each arm environment. Every arm result is then checked by the same
bidirectional checker, with the surrounding expected type when available.
This RFC does not add narrowing, exhaustiveness, or new pattern semantics.

## Diagnostics and recovery

Strict checking reports errors at the smallest expression whose synthesized
and expected types conflict. Contract errors retain the contract or declaration
as a secondary location. Generic call diagnostics distinguish conflicting
evidence from an underconstrained result.

Workspace recovery continues to represent missing and invalid syntax with its
existing explicit semantic fact states. Complete expressions use the same
resolved types as strict checking; recovery must not reinterpret an unsolved
inference variable as a source-level `Any` contract.

Cancellation is checked during expression traversal, unification, and block or
arm iteration so a superseded LSP query can stop cooperatively.

## Implementation plan

1. run the existing inference-variable checker unconditionally;
2. make it authoritative for final binding, result, and expression types;
3. add bidirectional handling for every expression form, especially `match`;
4. make block-local annotations and definition contracts provide expected
   types through the same path;
5. preserve provisional bootstrap inference only where tool-stage evaluation
   requires it;
6. resolve all recorded descriptors before interning the final type graph;
7. test ordinary modules without schemes, expected generic results, closures,
   blocks, conditionals, matches, monomorphic aliases, diagnostics, module
   interfaces, and cancellation;
8. run workspace tests, strict Clippy, formatting, and whitespace checks.

## Acceptance criteria

1. ordinary programs receive bidirectional checking even without a generic
   binding or imported scheme;
2. expected function contracts type closure parameters and results;
3. expected Array, Tuple, Struct, block, `if`, and `match` types reach nested
   expressions;
4. generic results can be solved from the surrounding expected type;
5. higher-order generic calls check callbacks using inferred parameter types;
6. conflicting and underconstrained generic calls fail deterministically;
7. direct generic references instantiate freshly while aliases remain
   monomorphic;
8. final semantic facts contain resolved types from the authoritative checker;
9. TypeMetadata construction and represented declared types remain unchanged;
10. no interface, trait, or associated-type representation is introduced;
11. runtime bytecode and calling conventions remain unchanged;
12. workspace tests and strict static checks pass.

## Deferred work

- flow-sensitive narrowing and exhaustive patterns;
- implicit generalization and a value restriction;
- explicit type application;
- constraints and interface or trait implementation selection;
- associated-type projections;
- higher-rank and higher-kinded types;
- richer static shapes for heterogeneous TypeMetadata constructors.

## Implementation result

The inference-variable checker now runs for every complete strict program,
independently of whether the module contains a generic scheme or imported
interface. Its resolved binding, result, and expression descriptors are the
authoritative final types; conservative shape inference remains only for the
existing tool-stage bootstrap.

Expected types flow through closures, calls, collections, blocks,
conditionals, and matches. Match arms use the same checker instead of falling
back to shape inference, and tuple patterns propagate known scrutinee member
types into their bindings. Nested binding annotations are evaluated as
TypeMetadata and supplied to block checking. Annotated bindings retain their
declared contract type rather than the narrower initializer type.

Known function arity errors and known non-`Type` metadata-constructor arguments
now fail during frontend checking in ordinary modules. Structural equality
remains intentionally heterogeneous and does not unify its operands. Failed
speculative branch unification restores inference substitutions before a union
result is formed.

Tests cover ordinary bidirectional checking without schemes, expected empty
collections and branch results, tuple-pattern propagation, nested annotation
failure, generic result inference, higher-order callbacks, monomorphic aliases,
module interfaces, semantic facts, and cancellation. The final workspace run
passed 191 core tests with one manual benchmark ignored, 9 CLI tests, and 19
LSP tests. Strict Clippy, formatting, and whitespace validation also pass.

## Rejected alternatives

### Keep generic inference conditional

Conditioning the stronger checker on whether a module happens to contain a
scheme makes unrelated declarations change expression typing. The same source
expression must follow the same rules in ordinary and generic modules.

### Treat unresolved variables as `Any`

An inference variable is an obligation created by a polymorphic contract;
`Any` is an explicit gradual boundary. Conflating them accepts calls whose
declared type relationship has not been established.

### Introduce associated types first

Associated projections require constraints, implementation selection, and
coherence rules that XL does not yet have. Structural bidirectional checking is
useful independently and provides the foundation on which such a system could
later be designed.
