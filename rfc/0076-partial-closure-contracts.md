# RFC 0076: Partial closure contracts

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Incomplete closure contracts no longer default to Any.
- Depends on: RFC 0050, RFC 0051, RFC 0052, RFC 0072, RFC 0073

## Summary

Closure parameters and results may carry optional TypeMetadata annotations:

```forma
fn(value: Int, context: Any) -> String {
    `\{context.name}: \{value}`
}
```

Each annotation supplies an input to bidirectional checking before the body is
analyzed. Unannotated positions retain ordinary inference variables:

```forma
fn(value: Int, other) { value + other } # Fn(Int, Int) -> Int
fn(value) -> Int { value + 1 }          # Fn(Int) -> Int
```

This is a local, monomorphic contract. Public generic contracts remain explicit
on `decl`, `def`, and `native` through `for(...)` and `Fn(...)`.

## Motivation

RFC 0073 makes intentional dynamic boundaries explicit, but a local closure
currently requires repeating its whole function shape on the binding:

```forma
let decorate: Fn(Any, Int) -> Int = fn(ctx, value) { ... };
```

Partial annotations place the information at the parameter or result it
constrains, allow the remaining positions to be inferred, and work for nested
or immediately passed closures that have no binding of their own.

## Syntax

The closure grammar becomes:

```text
closure   := 'fn' '(' [parameter (',' parameter)* [',']] ')'
             ['->' expression] block;
parameter := identifier [':' expression];
```

Annotation expressions use the existing TypeMetadata expression language.
They are evaluated in the same tool-stage environment and quota as local
binding annotations. Their source locations remain available for diagnostics.

`Fn` remains the function TypeMetadata constructor; lowercase `fn` remains a
value-producing lambda. This RFC does not restore named `fn` declarations.

## Checking

For each parameter:

- an annotation evaluates to a descriptor and becomes its expected type;
- otherwise an available surrounding `Fn` expectation supplies the type;
- otherwise the checker creates a fresh inference variable.

If both a local annotation and a surrounding function expectation exist, they
must be directionally compatible. A local annotation cannot silently override
the call-site or binding contract.

The optional result annotation is passed as the expected type of the closure
body. If a surrounding result expectation also exists, both must be compatible.
The published closure result uses the declared local result type, including
when the body has type `Never`.

Unannotated positions continue to participate in RFC 0072 and RFC 0073
constraint solving. No closure annotation introduces a type parameter or a
`TypeScheme`.

## Scope and evaluation

Annotation expressions are evaluated outside the closure's runtime parameter
scope. They may reference module bindings and TypeMetadata values available at
the closure definition, but not the closure's value parameters.

Annotations execute only during analysis/tool evaluation and are erased from
runtime bytecode except for metadata values otherwise used at runtime. They
share cancellation, fuel, allocation, and source-provenance behavior with
existing local annotations.

## Diagnostics and facts

Invalid metadata points to the annotation expression. A mismatch between a
local annotation and an enclosing contract labels both locations when
available. Body mismatches identify the body result and the local result
annotation.

Hir parameters retain their existing identities and locations. Hover and type
facts report the resolved parameter, body, and closure types; no syntax-only
annotation node becomes a runtime definition.

## Goals

1. support optional parameter annotations on closures;
2. support an optional closure result annotation;
3. accept any valid TypeMetadata expression used by local annotations;
4. combine partial annotations with surrounding expected function types;
5. infer every unannotated position normally;
6. keep annotations monomorphic and erased at runtime;
7. retain precise CST, AST, HIR, diagnostics, formatting, and semantic facts;
8. preserve query cancellation and tool-stage quotas.

## Non-goals

- implicit generic parameters or let-polymorphism;
- parameter annotations on `def` separate from its value closure;
- named, optional, variadic, or default arguments;
- dependent parameter types referencing earlier values;
- runtime reflection over source annotations;
- explicit generic type application, which belongs to RFC 0077.

## Implementation plan

1. extend the lossless grammar with annotated parameters and result arrows;
2. retain annotation expressions in AST closure parameters and closures;
3. index annotation references without defining runtime parameter visibility;
4. evaluate nested annotation metadata with existing tool-stage machinery;
5. merge local and surrounding expected descriptors in closure inference;
6. keep compiler parameter slots and captures unchanged;
7. update call-section synthesis to produce unannotated parameters;
8. add CST, parser, inference, conflict, invalid metadata, nested closure,
   quota, semantic-fact, and runtime-erasure tests;
9. update examples that become clearer with local dynamic annotations;
10. run full workspace tests and strict static checks.

## Acceptance criteria

1. a fully annotated closure checks and executes;
2. one annotated parameter constrains unannotated peers through the body;
3. a result-only annotation constrains parameters through the body;
4. omitted positions continue to infer;
5. explicit `Any` works at one parameter without erasing other positions;
6. local and surrounding compatible contracts compose;
7. conflicting contracts fail with annotation context;
8. invalid TypeMetadata is rejected at its annotation;
9. nested and immediately passed annotated closures work;
10. call-section closures remain source-equivalent and unannotated;
11. runtime arity, captures, and bytecode behavior do not change;
12. annotations consume shared tool quota and observe cancellation;
13. hover and binding facts expose the completed function type;
14. no implicit `TypeScheme` is created;
15. workspace tests and strict static checks pass.

## Deferred work

- explicit generic type application;
- monomorphic recursive SCC inference;
- named and default parameters;
- effect annotations;
- generic lambda binders.

## Implementation result

Implemented in the RFC 0076 change set.

The lossless grammar and AST now retain a `ClosureParameter` name plus optional
annotation and an optional closure result annotation. Parser recovery reports
damaged parameter annotations normally. HIR indexes annotation references in
the definition environment before introducing runtime parameters, while call
sections synthesize ordinary unannotated parameters.

The existing nested-annotation tool stage evaluates closure annotations under
the shared quota and stores their descriptors by source location. Closure
inference merges each local descriptor with any surrounding function
expectation, creates variables only for omitted positions, and checks the body
against the local or surrounding result descriptor. The compiler extracts only
parameter names, so runtime slots, captures, arity, and bytecode are unchanged.

Tests cover parameter-only, result-only, partial `Any`, nested, surrounding,
conflicting, and invalid metadata contracts plus direct execution and erasure.
The final workspace run passed 248 Forma tests with one manual parser benchmark
ignored, 12 CLI tests, and 19 LSP tests.

## Rejected alternatives

### Require only whole-binding function annotations

Whole contracts remain useful for interfaces, but they are cumbersome for
nested callbacks and cannot express a partial local hint without repeating all
inferred positions.

### Use a separate restricted type grammar

Forma types are TypeMetadata values. Reusing annotation expressions preserves
computed types and avoids creating two notions of what a local type may be.

### Generalize annotated variables implicitly

An annotation constrains one monomorphic closure instance. Generic behavior is
still declared explicitly with `for(...)` at a binding contract.
