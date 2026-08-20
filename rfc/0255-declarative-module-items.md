# RFC 0255: Declarative Module Items

- Status: Accepted
- Tracking issue: #114
- Supersedes: module-level `let`, `export let`, and authored module results
- Depends on: RFC 0226, RFC 0235, RFC 0238

## Summary

Telora modules are declarative namespaces, not ordinary expression blocks.
Their top level accepts only module items:

```text
option | import | type | decl | def | native | export
```

`let` remains a lexical block binding. A module value that needs local,
ordered computation uses an ordinary `do` expression as its `def`
initializer:

```telora
def answer: Int = do {
    let base = 40;
    base + 2
};

export {answer};
```

Modules no longer accept top-level `let`, `export let`, a bare expression, or
a final result expression. Every source module continues to publish at least
one explicit export.

## Motivation

The old surface mixed two models. The parser treated a module body much like a
block and retained an authored final expression, while module loading required
an explicit export record. Top-level `let` additionally mixed lexical
sequencing and shadowing with module slots and dependency scheduling.

This ambiguity repeatedly caused test and scratch modules to use calls as
top-level statements, depend on unexported eager evaluation, or fail with the
parser-internal `binding has no name` diagnostic. It also made the intended
difference between `let` and `def` difficult to explain.

## Surface grammar

The authoritative module grammar is:

```text
Module       := ModuleItem* EOF
ModuleItem   := Option | Import | Type | Decl | Def | Native | Export
```

An ordinary block retains lexical bindings followed by one result expression:

```text
Block        := "{" BlockBinding* Expression "}"
BlockBinding := Let | LetPattern | LetElse | local forms already admitted by
                the block grammar
```

This RFC does not introduce statements. Runtime computation remains
expression-based; module items are declarations that construct a module
namespace and export interface.

`export def`, `export type`, and `export { ... }` remain. `export let` is
removed. A source module with no explicit exported member is invalid.

## Module and block semantics

`def` is the ordinary module value form. It owns one module binding, cannot
shadow another explicit module binding, and participates in the module
dependency graph. Its initializer is an expression and may therefore be a
`do` block containing local `let` bindings.

`let` is lexical. It expresses ordered local computation, introduces a name
only in its block, and may shadow according to the existing lexical rules. It
does not allocate a module slot or enter a module interface.

All explicit module names share one namespace. A selective, aliased, or
namespace import conflicts with a same-named `type`, `def`, `decl`, or native
binding. Open imports and the implicit prelude remain fallback candidates and
are ignored when an explicit module item owns the name.

Exports select already-defined local module names. They do not execute an
expression, introduce a lexical alias, or make source order observable.

## Tool behavior

`check` validates and evaluates the export graph of a real module. It does not
accept a no-export fragment merely for scratch use. Independent exported
checks are the recommended best-effort diagnostic roots:

```telora
export def parse_case = validate_parse.must_ok!(input);

export def lowering_case = do {
    let parsed = parse(input);
    validate_lowering.must_ok!(parsed, expected)
};
```

`run` additionally applies the selected Entry contract to the complete export
record. `show` and LSP may expose recovered facts from an incomplete module,
but recovery success does not make that module valid.

Diagnostics distinguish the removed forms:

- top-level `let` says that lexical `let` is allowed only in a block and
  suggests `def`, or `def name = do { ... }` for ordered initialization;
- a top-level expression says that expression statements are unsupported and
  suggests binding and exporting the intended result;
- `export let` suggests `export def`;
- a module without exports explains that `check` requires a real module
  interface.

No path reports the parser-internal `binding has no name` for these authored
forms.

## Migration

Existing top-level `let name = expression;` becomes `def name = expression;`.
Existing `export let` becomes `export def`. When the initializer needs several
lexical steps it becomes one `do` expression. Function and nested block `let`
bindings are unchanged.

The migration intentionally has no compatibility mode. Parser recovery may
recognize removed syntax to provide a focused diagnostic, but it cannot lower
that syntax as a valid module item.

## Implementation plan

1. Split authoritative parsing of module items from ordinary block bodies.
2. Remove authored module results and synthesize the result exclusively from
   explicit export markers.
3. Reject top-level `let`, `export let`, and top-level expressions with focused
   diagnostics in strict and recovery parsing.
4. Make HIR, type analysis, module skeleton construction, and compilation rely
   on the declarative module invariant.
5. Update tree-sitter, queries, formatter behavior, parser baselines, and LSP
   recovery coverage.
6. Migrate repository modules and documentation, including experiment input
   tutorials.

## Rejected alternatives

### Allow no-export modules only in `check`

That would make `check` success insufficient evidence that its input is a
loadable module and would encourage tests to depend on evaluation of
unpublished roots.

### Treat top-level calls as implicit checks

This adds statement semantics and makes evaluation depend on source order.
Explicit exported values already provide named, independently recoverable
diagnostic roots.

### Keep top-level `let` as a private module binding

`def` already expresses private module values. A second module value form
would preserve ambiguous shadowing, inference, and dependency semantics.

### Keep a module final expression as a default export

It conflicts with the explicit Module interface and makes `run`, `check`, and
imports observe different publication conventions.

## Acceptance criteria

1. Top-level `let`, `export let`, bare expressions, and final expressions are
   rejected with focused, sourced diagnostics.
2. `def value = do { let local = ...; result }` is accepted and preserves the
   result type and provenance.
3. `let` remains valid inside functions and nested `do` blocks, including
   lexical shadowing.
4. `def`, `type`, explicit imports, declarations, and native bindings cannot
   collide in the module namespace; recovery does not overwrite their HIR
   identities.
5. `check` still requires a nonempty explicit interface; `run` still applies
   its Entry contract; `show` and LSP retain recovery facts.
6. Tree-sitter, formatter, LANGUAGE SSOT, main tutorial, CLI tutorial, and
   experiment tutorial agree with the authoritative grammar.
7. All repository Telora modules use `def` for top-level values and contain no
   `export let`.
8. Parser, core, CLI, LSP, tree-sitter, formatting, and workspace tests pass.
