# RFC 0218: Nameable parameterized TypeMetadata families

- Status: Implemented
- Depends on: RFC 0051, RFC 0055, RFC 0192, RFC 0203

## Summary

Add parameterized `type` declarations whose result can be named precisely in
another generic contract:

```telora
@struct
type Box(A) = {
    value: A,
};

def wrap: for(A) Fn(A) -> Box(A) = fn(value) {
    {value}
};

def box_type:
    for(A) Fn(TypeOf(A)) -> TypeOf(Box(A)) =
    Box;
```

`Box` has one coherent surface in type and value positions:

```text
type position    Box(A)
value position   Box : for(A) Fn(TypeOf(A)) -> TypeOf(Box(A))
```

The declaration evaluates its decorated body once with rigid symbolic
TypeMetadata parameters and records the resulting canonical metadata template.
Applying the family substitutes supplied metadata into that template. It does
not re-run arbitrary user code for each concrete type.

This RFC adds neither traits nor a general kind system. It is the narrow
missing relation identified by RFC 0203: executable TypeMetadata construction
already works, but a user-defined family result cannot currently be named as
`TypeOf(F(A))` inside another generic contract.

## Current gap

An ordinary metadata function can construct a concrete type:

```telora
def Box:
    for(A) Fn(TypeOf(A)) -> Type =
    fn(A) {
        struct('None, {value: A})
    };

type IntBox = Box(Int);
```

The closed declaration `IntBox` receives the complete generated descriptor.
The function's reusable contract nevertheless widens its result to `Type`.
There is no binding available to the annotation evaluator that can denote the
relationship between `A` and the generated record:

```telora
# Not expressible today.
def Box:
    for(A) Fn(TypeOf(A)) -> TypeOf(Box(A)) = ...;
```

Higher-order consumers can preserve safety by accepting the already closed
record type plus typed constructors and selectors. The ontology experiments
proved that fallback, but also made its cost visible through long positional
interfaces and mechanical forwarding closures. That experiment supplies
evidence; it does not define this RFC's vocabulary or semantics.

Neutral container, protocol-envelope, and codec-record families have the same
missing relationship. The language gap is therefore about composable
TypeMetadata witnesses, not ontology or graph behavior.

## Surface syntax

A parameterized type declaration adds one or more identifiers after the type
name:

```text
type_binding:
    decorator* 'type' Identifier
    ['(' Identifier (',' Identifier)* [','] ')']
    '=' expression ';'
```

Examples:

```telora
@struct
type Box(Item) = {value: Item};

@struct
type Envelope(Payload, Error) = {
    payload: Option(Payload),
    error: Option(Error),
};
```

An empty parameter list is rejected. Duplicate parameters are rejected at the
duplicate location. An ordinary unparameterized `type Name = expression`
retains its existing semantics.

Decorators apply to the symbolic body result, exactly once, before the family
template is published. They are not re-executed at each application.

## Symbolic template semantics

For:

```telora
type Family(A, B) = body;
```

the tool stage performs these steps atomically:

1. allocate rigid `TypeParameterId` identities for `A` and `B`;
2. bind each parameter to the existing TypeMetadata encoding of
   `TypeDescriptor::Bound`;
3. evaluate the decorated `body` once with the ordinary tool-stage VM;
4. decode the result as canonical TypeMetadata;
5. verify that every free Bound in the template belongs to this declaration;
6. publish the template and its constructor scheme together.

The resulting value binding has the scheme:

```text
Family:
    for(A, B)
    Fn(TypeOf(A), TypeOf(B))
        -> TypeOf(template[A, B])
```

The source spelling `Family(A, B)` names `template[A, B]` in a contract. Static
facts and diagnostics may display the normalized structural result rather than
preserving the alias spelling. This RFC does not add a nominal family identity
to runtime values or structural assignability.

The body is evaluated over symbolic parameters, not concrete examples. Code
that observes a parameter through `TypeDesc` therefore observes the existing
public `Bound` descriptor during declaration evaluation. Its branch is fixed
in the published template. A family application never re-runs that branch on
the concrete argument.

This rule is required for sound generic contracts. If arbitrary family code
were re-run for `Int` after its generic result had been inferred from `Bound(A)`,
the concrete execution could choose a different descriptor than the scheme
promises.

## Family application

Applying a family value:

```telora
Family(Int, String)
```

performs capture-avoiding substitution of the supplied descriptors for the
family's declared Bound identities. Every argument must decode as valid
TypeMetadata. The result is canonical TypeMetadata for the substituted
template.

Application is available wherever ordinary closed TypeMetadata computation is
currently available, including:

- the right-hand side of another type declaration;
- a definition, closure-parameter, or result contract;
- `TypeOf(Family(A))` inside an explicit generic scheme;
- a decorator or metadata-constructor argument; and
- ordinary program code that deliberately uses TypeMetadata as a value.

The callable uses the ordinary Function ABI. Type arguments remain metadata
values; no runtime kind argument, specialization, dictionary, or family
dispatch is added.

Partial application is not added. A family call supplies exactly its declared
number of TypeMetadata arguments.

## Static semantics

The family constructor scheme is authoritative. Inference and explicit type
application instantiate it through the existing rank-1 machinery:

```telora
Box(Int)       # TypeOf(Box(Int))
Box[A](A)      # TypeOf(Box(A)) inside a generic definition
```

The second form uses the existing explicit scheme application syntax. It does
not make `Box` a higher-kinded parameter or a first-class `TypeScheme` value.

The declaration body itself is required to evaluate to valid TypeMetadata.
Known non-Type inputs to metadata constructors retain ordinary static errors;
malformed or dynamically imprecise results retain authoritative decoder
errors. The decoded symbolic template is the final check.

Two applications are assignable according to their normalized substituted
descriptors. Two different family declarations producing equal structural
metadata do not become nominally distinct.

## Dependency and recursion boundary

Parameterized family declarations may depend on:

- built-in metadata constructors;
- imported metadata values and families;
- ordinary closed helper functions available at tool stage; and
- other local parameterized families in an acyclic dependency component.

The implementation evaluates local family declarations in deterministic
dependency order rather than source order.

A recursive component containing a parameterized family is rejected in this
RFC. This includes direct recursion, mutual family recursion, and recursion
through a helper needed to construct the template. Existing unparameterized
recursive TypeMetadata remains unchanged.

Parameterized recursive families require separate semantics for applying
arguments through finite graph back-edges. They must not be approximated with
`Any`, eagerly unfolded, or admitted accidentally by the existing recursive
type predeclaration path.

## Modules and interfaces

An exported family publishes:

- its ordinary callable value;
- its explicit parameter names and stable scheme identities;
- the precise constructor result template; and
- its source provenance.

Whole-module, selective, open, and aliased imports preserve the same scheme and
application behavior. Import style must not widen the result to `Type`, erase
the template to `Any`, or change whether a contract is accepted.

The family callable and any captured template data use the existing persistent
publication model. Failed template construction, recursive-family rejection,
cancellation, quota exhaustion, or stale workspace evaluation publishes no
partial family interface.

## Diagnostics and tooling

Diagnostics must distinguish:

- duplicate family parameters;
- wrong application arity;
- a non-TypeMetadata argument;
- an invalid symbolic body result;
- a free or foreign Bound in the result template;
- a recursive family component; and
- an application whose substituted result conflicts with its context.

Declaration failures point to the family declaration and the failing body or
decorator expression. Application failures point to the application and retain
the declaration as a related source when it contributes to the cause.

HIR definitions record family type parameters. Semantic facts, hover, CLI type
display, module interfaces, recovery, and LSP completion expose the precise
constructor scheme. No unresolved inference identity, private Bound number, or
partial template may enter a published snapshot.

Incomplete family declarations participate in existing recovery. They may
block dependent contracts without erasing independent module facts and must
not fabricate a plausible family scheme.

## Runtime and implementation boundary

This RFC requires one general operation: substitute validated TypeMetadata
arguments into a published symbolic descriptor template. The implementation
may realize it with a private core native, a synthetic closure, or an equivalent
internal mechanism using the existing Function ABI.

It does not add:

- a public VM instruction or family object protocol;
- a second TypeMetadata evaluator;
- generated executable code;
- runtime type argument inference;
- generic specialization; or
- a standard-library type-family registry.

The symbolic body is evaluated by the existing VM. Substitution reuses the
same capture-avoiding descriptor operation already required by generic scheme
instantiation.

## Goals

1. let user code name a parameterized metadata result precisely in another
   generic contract;
2. preserve the relationship through ordinary rank-1 inference and explicit
   application;
3. keep declaration evaluation programmable and source-aware;
4. make family application deterministic and sound by substituting one
   symbolic template;
5. preserve exact schemes across every static module import form;
6. retain the existing runtime TypeMetadata representation and Function ABI;
7. improve eDSL APIs without introducing domain vocabulary into generic
   libraries; and
8. produce precise declaration, application, recovery, CLI, and LSP facts.

## Non-goals

- arbitrary value-dependent types or arbitrary function calls in type syntax;
- parameterized recursive type families;
- higher-kinded types or passing a family as a type parameter;
- traits, interfaces, associated types, type classes, or instance search;
- structural constraints, row polymorphism, field-shape inference, or
  automatic selector generation;
- nominal or generative type constructors;
- partial family application or variadic type parameters;
- proving arbitrary user functions pure or total;
- ontology, analytics, graph, build, deployment, or Agent vocabulary in the
  language or generic standard library; or
- removing every typed callback from an eDSL API.

## Stopping rules

Stop implementation and return to design discussion if the accepted surface
requires any of the following:

1. evaluating the body separately for each concrete argument to recover its
   result relationship;
2. admitting a recursive family through eager expansion or `Any`;
3. treating arbitrary ordinary functions as type-position bindings;
4. adding higher-kinded unification, associated-type projection, trait search,
   subtyping, or structural constraints;
5. publishing a local-only family whose scheme cannot survive every existing
   static import form;
6. introducing a second evaluator or domain-specific TypeMetadata protocol;
7. exposing internal Bound identities as a public runtime ABI; or
8. accepting a family whose generic scheme disagrees with its value-level
   application.

These indicate that the proposed mechanism is not the bounded feature defined
by this RFC.

## Acceptance criteria

1. an undecorated neutral `Box(A)` family is usable in value and contract
   positions with a precise witness;
2. a decorated two-parameter `Envelope(Payload, Error)` family preserves both
   parameters through a nested `Option` or `Result` shape;
3. one family composes another and the complete normalized result remains
   precise;
4. a generic constructor and consumer use `Family(A)` without `Any`, `Dyn`, a
   selector, or a continuation workaround;
5. whole-module, selective, open, and aliased imports preserve the same family
   scheme and accepted program;
6. concrete applications evaluate to canonical TypeMetadata and validate
   corresponding values;
7. duplicate parameters, arity mismatch, invalid metadata, foreign Bound, and
   recursive family components produce stable sourced diagnostics;
8. hover, CLI, LSP, strict analysis, and recovered workspace facts publish no
   erased or provisional family scheme;
9. existing unparameterized, computed, decorated, recursive, imported, codec,
   schema, and interpreter TypeMetadata behavior does not regress; and
10. full workspace tests, formatting, and strict Clippy pass.

At least two acceptance fixtures must use neutral non-ontology vocabulary. An
ontology experiment may be retained as a downstream regression but is not
required to explain or validate the mechanism.

## Rejected alternatives

### Allow arbitrary metadata functions in type position

An arbitrary `Fn(Type) -> Type` does not state which result type corresponds
to its argument. Running it once with `Bound(A)` and again with `Int` can take
different reflection branches. Treating those results as one generic relation
would be unsound; treating every function call as a dependent type expression
would add substantially broader evaluation, dependency, and termination
semantics.

### Re-run the family body for every application

This appears closest to ordinary functions but permits concrete TypeDesc
inspection to disagree with the symbolic generic scheme. A single published
template plus substitution makes the relationship deterministic and
inspectable.

### Add traits or associated types

No implementation search or behavior dispatch is required. The missing fact
is one metadata result relation. Traits would add coherence, resolution, and
associated projection before this smaller mechanism is tested.

### Keep using typed selectors and continuations

That fallback is sound and remains useful where APIs genuinely abstract over
unrelated closed records. It does not make the generated type family itself
nameable, and experiments have shown substantial mechanical API width when
the family relationship is the only missing fact.

### Add a broad kind system

Families are not accepted as generic type parameters in this RFC. Their
declared parameters are ordinary TypeMetadata witnesses and their application
produces ordinary TypeMetadata. A separate `Type -> Type` kind hierarchy would
add machinery without serving the bounded surface.

## Implementation result

Implemented in `telora-core` with the existing parser, analyzer, compiler, VM,
module publication, and semantic snapshot paths. A family is represented by an
ordinary native closure whose upvalues contain the canonical symbolic
TypeMetadata value, including attribute wrappers. Application validates
TypeMetadata arguments and performs schema-aware, capture-avoiding Bound
substitution while retaining the authored application location on the produced
rich metadata graph. No public Value variant, bytecode instruction, ABI,
evaluator, or standard-library registry was added.

Local families are evaluated in deterministic dependency order. The first
implementation accepts acyclic composition between local families and imported
metadata capabilities. It deliberately rejects dependency on an
unparameterized `type` declared in the same module: those declarations still
participate in the older recursive predeclaration path, whose temporary `Any`
placeholder must not enter a family template. Local ordinary helpers are not
yet available during this early family-evaluation step. Supporting either case
requires a later dependency-scheduling change; it is not approximated here.

A post-implementation correction preserves top-level and member attributes,
codec policy, and rule provenance through application. Concrete type
declarations in the same module dereference a family as a callable rather than
passing its metadata up-link to the call. Partial workspace analysis binds rigid
family parameters, publishes the exact constructor scheme and callable, and
projects that scheme when strict analysis is unavailable because of an
independent error.

Tests cover located and duplicate parameters, exact schemes, decorated and
nested composition, forward acyclic dependencies, concrete validation, all
four static import forms, attribute and codec preservation, authored rule
provenance, same-module concrete application, wrong arity, invalid metadata,
local concrete-type rejection, direct and mutual recursion rejection, and
precise complete and partial recovered semantic facts. Full core and workspace
test suites pass. On Rust 1.97, strict workspace Clippy also passes; its two
newly surfaced baseline lints were resolved by boxing a private recovery error
payload and removing a redundant single-variant diagnostic-dispatch argument.

## SSOT delta

The implementation updates these documents in the same work:

- `docs/design/LANGUAGE.md`: add parameterized type declarations, symbolic
  template evaluation, precise constructor schemes, module behavior, and the
  retained rank-1/non-trait boundaries;
- `docs/design/CONCEPT.md`: define Type Family and distinguish it from an
  arbitrary metadata function, associated type, and higher-kinded parameter;
- `tutorial.md`: teach the declaration, contract, import, and diagnostic
  surface with neutral examples; and
- this RFC: records the exact implementation, tests, and narrowed boundary.

`docs/MOTIVATION.md` does not change. The feature follows its existing claims:
simple explicit declarations, types as data, diagnostics as a first-class
requirement, selective PLT adoption, and domain behavior remaining in eDSL
libraries.
