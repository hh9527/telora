# RFC 0073: Delayed monomorphic local inference

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Unresolved monomorphic evidence requires context; generic schemes preserve Bound relationships.
- Depends on: RFC 0052, RFC 0070, RFC 0071, RFC 0072

## Summary

Unannotated `let` and `def` bindings may retain monomorphic inference
obligations through the remainder of their lexical block. Later uses solve the
same obligations:

```forma
let identity = fn(value) { value };
let number = identity(1);
```

infers:

```text
identity: Fn(Int) -> Int
number: Int
```

The binding is not generalized. A conflicting later use fails:

```forma
identity("text"); # Int conflicts with String
```

At block completion every retained variable must be concrete. A binding that
remains underconstrained reports an error and must use an explicit `Any`
contract if dynamic behavior is intended.

## Motivation

RFC 0072 proves that closure parameters can use ordinary inference variables,
but a closure whose body only expresses a relationship cannot solve itself:

```forma
fn(value) { value } # Fn(?A) -> ?A
```

The existing immediate `Any` fallback loses that relationship before a nearby
call can contribute evidence. This is especially visible in ordinary helpers,
call sections, callbacks, and pipeline functions.

Forma does not need implicit polymorphism to solve this problem. It needs one
monomorphic variable identity shared by the initializer and every later use in
the same block.

## Goals

1. retain unresolved variables from an unannotated `let` or `def` initializer
   through the remainder of its lexical block;
2. let later direct calls, aliases, fields, callbacks, and structural contexts
   solve those variables;
3. create a fresh item obligation for an unannotated empty Array binding;
4. preserve one monomorphic solution across all uses;
5. reject conflicting uses deterministically;
6. reject bindings that remain underconstrained at block completion;
7. require an explicit `Any` annotation for intentionally dynamic functions or
   collections;
8. resolve binding types and expression facts only after block constraints are
   complete;
9. prevent inference variables from crossing module interfaces;
10. preserve explicit `for(...)` as the only source-level generalization;
11. retain cancellation checkpoints during delayed solving and finalization.

## Non-goals

- implicit generalization or let-polymorphism;
- a value restriction;
- polymorphic recursion or inference of recursive definitions;
- solving a binding from uses before its declaration;
- carrying unresolved variables across module boundaries;
- whole-program or workspace-global constraint solving;
- overload resolution, subtyping, coercion, or numeric defaulting;
- inferring open Struct shapes from field access;
- changing explicit `Any` behavior.

## Binding scope

Only an unannotated value binding establishes a delayed obligation:

```forma
let value = expression;
def value = expression;
```

An annotation supplies the completion boundary immediately:

```forma
let identity: Fn(Int) -> Int = fn(value) { value };
```

`decl`, `native`, `type`, and `import` bindings retain their existing contracts
and interface behavior.

The obligation lives from the initializer through the final expression of the
same lexical block. Nested blocks have independent completion boundaries.

## Shared monomorphic identity

Given:

```forma
let identity = fn(value) { value };
let alias = identity;
alias(1)
```

the initializer, `identity`, and `alias` all reference the same inference
variable identity. The call solves it once as `Int`.

Later uses do not instantiate fresh variables:

```forma
identity(1);
identity("text");
```

The second call conflicts. To obtain polymorphism, the author writes:

```forma
def identity: for(A) Fn(A) -> A = fn(value) { value };
```

## Empty Array bindings

Within an unannotated delayed binding:

```forma
let values = [];
```

the initializer has `Array(?A)`, not `Array(Any)` or `Array(Never)`. A later
use may solve it:

```forma
native append: for(A) Fn(Array(A), A) -> Array(A);
let values = [];
append(values, 1) # values: Array(Int)
```

If no later evidence exists, block completion reports an underconstrained
binding. Other wholly unconstrained literal forms retain their existing rules
unless their structural expected type already carries variables under RFC
0071.

## Higher-order uses

Expected callback types constrain a delayed closure:

```forma
native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);

let identity = fn(value) { value };
map([1, 2], identity)
```

solves `identity` as `Fn(Int) -> Int` and the call as `Array(Int)`.

Storing a closure in a Struct and later calling the selected field preserves
the same inference identities because structural descriptors carry those
variables without re-instantiation.

## Block completion

After the block result has been checked, the checker resolves every delayed
binding. If any inference variable remains, analysis fails:

```text
cannot infer monomorphic binding `identity`: unresolved Fn(?0) -> ?0
```

The binding initializer is the primary location. The diagnostic may name the
first unresolved parameter when available.

This is intentionally stricter than RFC 0072's closure-local compatibility
fallback. An author who intends a dynamic contract writes it explicitly:

```forma
let identity: Fn(Any) -> Any = fn(value) { value };
```

The annotation makes the loss of guarantees visible to readers, interfaces,
hover, and future optimizers.

## Top-level and nested blocks

The module body is a block and follows the same completion rule. Its delayed
bindings may be solved by later top-level bindings or the module result.

A nested block completes before control returns to its parent. Its unresolved
variables cannot escape indirectly through a returned value; returning an
underconstrained local function is therefore an error unless an expected type
from the parent solves it while the nested block is checked.

## Recursive definitions

This RFC does not infer recursive definitions from their self-calls. Existing
bootstrap slots may make the name callable, but a recursive function whose
contract cannot be determined from its body and non-recursive uses requires an
explicit annotation.

This avoids SCC-wide constraint solving and polymorphic-recursion questions.

## Generic calls

A generic call result may remain temporarily unresolved when its variables are
linked to a delayed local argument:

```forma
def observe = fn(value) { generic_identity(value) };
```

The relationship remains part of the binding obligation. A generic result with
no argument or expected-result evidence still fails at the call site, as in
RFC 0052.

## Diagnostics and recovery

Conflicting use sites retain the existing smallest-expression incompatibility
diagnostic. The binding initializer is added as secondary context where
practical.

An unresolved completion diagnostic distinguishes a delayed inference variable
from explicit `Any`. Recovery may publish an unknown semantic fact for the
affected binding, but must not reinterpret `?A` as a completed `Any` fact.

Cancellation is checked while traversing bindings, calls, and final unresolved
obligations. A cancelled or stale query publishes neither partial substitutions
nor partially resolved facts.

## Semantic facts and interfaces

Expression records may contain inference variables internally while the block
is being analyzed. Publication resolves every record through the completed
substitution map.

No `InferenceVariableId` reaches `TypeGraph`, `WorkspaceTypeGraph`, module
interfaces, CLI type output, or LSP hover. Unannotated exported functions remain
monomorphic; this RFC does not synthesize a quantified `TypeScheme` for them.

## Implementation plan

1. add an explicit delayed-binding scope to `GenericInference`;
2. suppress RFC 0072's unresolved closure fallback only while checking an
   unannotated `let` or `def` initializer;
3. insert the unresolved monotype into the block environment;
4. let later uses constrain the same descriptor and substitutions;
5. create a fresh item variable for an empty Array initializer in a delayed
   binding;
6. finalize delayed bindings after the block result is inferred;
7. report unresolved bindings before any descriptor is interned or published;
8. resolve top-level `binding_types` and all expression records after
   finalization;
9. add identity, alias, callback, field, empty Array, conflict, explicit `Any`,
   nested block, recursive, semantic-fact, and cancellation tests;
10. migrate intentionally dynamic existing fixtures to explicit `Any`
    contracts;
11. run full workspace tests and strict static checks.

## Acceptance criteria

1. a later call infers `fn(value) { value }` monomorphically;
2. aliases preserve and solve the original variable identity;
3. a higher-order generic callback can solve a delayed closure;
4. a closure stored in a Struct can be solved through a later field call;
5. an empty Array binding is solved by a later element-bearing use;
6. conflicting later uses fail deterministically;
7. an unused identity closure reports an unresolved binding;
8. an unused empty Array reports an unresolved binding;
9. an explicit `Fn(Any) -> Any` closure remains valid and dynamic;
10. no implicit `TypeScheme` is created;
11. recursive inference remains annotation-required when otherwise unresolved;
12. nested block obligations cannot escape unresolved;
13. final binding types and expression facts contain no inference variables;
14. module interfaces contain no inference variables;
15. cancellation prevents partial publication;
16. workspace tests and strict static checks pass.

## Deferred work

- inferred generalization and a value restriction;
- SCC-based recursive inference;
- parameter annotation syntax;
- explicit type application;
- subtyping, narrowing, and traits.

## Implementation result

Implemented in the RFC 0073 change set.

`GenericInference` now marks unannotated binding initializers as delayed.
Closure parameters and empty Array elements created in that region retain their
inference identities, and later lexical uses constrain those same monotypes.
Generic calls may also retain an unresolved result while such an initializer is
being checked.

Completion checks only variables created by the initializer. This ownership
rule is important for aliases such as `let y = x`: a nested binding may carry an
outer obligation without prematurely completing it. Top-level binding
descriptors, result descriptors, and expression records are resolved before
they are interned or published.

Existing tests that intentionally cross dynamic boundaries now state their
`Any` contracts explicitly. This includes decorator contexts, debug and
function-identity callbacks, and helpers used at both metadata and runtime
types. Direct calls, aliases, higher-order callbacks, Struct fields, empty
Arrays, conflicts, underconstrained nested blocks, explicit `Any`, and
unannotated recursion are covered by focused tests.

## Rejected alternatives

### Generalize unresolved local variables

That would make `identity` polymorphic and require scheme instantiation, a
value restriction, recursive rules, and observable compatibility decisions.
Forma already has explicit `for(...)` contracts.

### Keep defaulting every unresolved binding to Any

This silently destroys relationships that later uses could solve and makes an
accidental lack of evidence indistinguishable from an intentional dynamic API.
After delayed solving exists, dynamic behavior should be explicit.

### Solve across module boundaries

Module interfaces must be deterministic products of the imported module, not
depend on the importing module's use sites. Every local obligation completes
before interface publication.

### Infer recursive SCCs now

Recursive inference needs a separate dependency analysis and raises
polymorphic-recursion and diagnostic-order questions. Explicit contracts are a
clear boundary while ordinary forward local uses are addressed.
