# RFC 0041: Resolved HIR and expression facts

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Incomplete expression and definition evidence is absent, rather than an Any fact.
- Depends on: RFC 0039, RFC 0040

## Summary

XL introduces a source-shaped resolved HIR for complete, valid programs. The
HIR assigns compact identities to definitions, references, and expressions,
records lexical name resolution once, and carries no runtime values. Type
analysis records expression results against HIR expression IDs. Workspace
semantic queries are derived from this shared HIR rather than running their own
scope resolver.

The HIR is initially an identity and fact layer beside the located semantic
AST, not a second owned expression tree:

```rust
struct HirProgram {
    definitions: Vec<HirDefinition>,
    references: Vec<HirReference>,
    expressions: Vec<HirExpression>,
}

struct HirReference {
    location: Location,
    resolution: HirResolution,
}

enum HirResolution {
    Definition(HirDefinitionId),
    External,
    Unresolved,
}
```

The type checker and compiler receive the same resolved program. They may keep
name-keyed environments as local implementation details, but neither defines a
second user-visible resolution result. `WorkspaceSnapshot` remaps module-local
HIR IDs into its global identity space.

## Motivation

RFC 0039 made workspace semantics observable, but its `SemanticIndexer`
reimplemented lexical scopes while walking the AST. Compilation and type
analysis also maintain name-keyed environments. Continuing toward partial
snapshots or LSP over this arrangement would create three opportunities for
scope and shadowing behavior to drift.

The current `type_at` is also intentionally narrow. It knows top-level binding
roots, references to those bindings, and module results, but cannot answer the
type of literals, calls, field expressions, pipelines after elaboration,
branches, or nested expressions.

Resolved HIR provides one identity vocabulary before recovery, revisions, and
incrementality add lifecycle complexity.

## HIR identity

HIR IDs are compact indices local to one `HirProgram`:

```rust
struct HirDefinitionId(u32);
struct HirReferenceId(u32);
struct HirExpressionId(u32);
```

They are deterministic for identical AST input but are not stable across
reparse or rebuild. Workspace IDs remain a separate snapshot-local namespace.

Every HIR record retains its source location. Definitions additionally retain
all declaration/initialization name locations belonging to one definition
slot. Expressions identify the complete semantic expression range. A variable
expression therefore has both an expression ID and a reference ID.

The HIR covers:

- top-level and nested bindings;
- `decl`/`def` single-assignment slots;
- recursive named functions and type predeclarations;
- closure parameters;
- match pattern bindings;
- decorators as their already-elaborated RHS calls;
- every semantic expression recursively.

## Name resolution

The resolver follows existing XL scope semantics:

- ordinary `let` and import bindings enter scope after their RHS;
- `decl`, named functions, native bindings, and types are predeclared in their
  block;
- `def` initializes and shares its matching declaration identity;
- closure parameters and match bindings are local to their bodies;
- inner blocks shadow ordinary outer bindings where the language permits it;
- prelude and host-supplied bindings resolve as `External`;
- an unknown name remains `Unresolved` with its exact source location.

Complete compilation rejects unresolved HIR references through the existing
frontend diagnostic boundary. The unresolved representation is retained so a
future recoverable HIR can publish partial facts without inventing definitions.

Field names are not lexical references. Static field/member navigation remains
a type-query concern and is deferred.

## Compiler boundary

Normal program analysis constructs the resolved HIR once and stores it in
`Analysis`. Compilation validates and consumes that same HIR. Register
allocation may continue using source names because registers are local to an
already-resolved lexical scope; those maps are not exposed as semantic name
resolution.

Compiler-generated programs, including MetadataInit and isolated tool
expressions, run the same HIR resolver over their synthetic AST before
compilation. They do not manufacture ad hoc resolution records.

Free-variable/capture optimization may remain name-based in this RFC provided
its result cannot change reference identity. Moving capture sets to definition
IDs is deferred until it removes an observed ambiguity or enables caching.

## Expression type facts

Type inference records the inferred `TypeDescriptor` for every visited semantic
expression while it performs the existing analysis. After the module
`TypeGraph` is built, descriptors are interned and stored as:

```rust
BTreeMap<HirExpressionId, TypeId>
```

Facts are observational results of the existing inference rules; this RFC adds
no narrowing, unification, subtyping, or bidirectional inference. An expression
whose existing rule yields `Any` records `Any` rather than absence.

Promoted TypeMetadata remains authoritative for declared type roots. References
query their resolved definition type before a conservative bootstrap expression
fact, so forward and recursive type names do not regress to the bootstrap
`Any` shadow in tooling.

## Workspace projection

Snapshot construction remaps each module's HIR records into global workspace
definition, reference, and expression IDs. The former independent
`SemanticIndexer` is removed.

The workspace adds:

```text
expressions()
expression(id)
expression_at(location)
type_of_expression(id)
```

`type_at(location)` resolves in this order:

1. the narrowest variable reference and its definition type;
2. the narrowest definition name and its definition type;
3. the narrowest expression with a recorded type;
4. explicit absence.

This makes hover selection predictable while preserving authoritative types for
names. Position lookup remains read-only and executes no XL code.

## CLI observation

`xl show` adds an `expressions:` section containing expression ID, source
location, and known type. `xl show ... at ...` reports the narrowest expression
and its type in addition to definition/reference information.

The output is diagnostic tooling, not a stable serialization format.

## Non-goals

- invalid or incomplete source recovery;
- open-document versions and workspace overlays;
- incremental HIR IDs or caching;
- an owned HIR expression tree replacing AST in the compiler;
- field/member definition resolution;
- more powerful type inference;
- LSP transport.

## Acceptance criteria

1. one shared resolver produces definitions, references, and expressions for a
   valid program.
2. query construction contains no independent lexical scope resolver.
3. `decl`/`def`, recursion, shadowing, parameters, patterns, imports, and
   external names retain correct resolution.
4. compiler and type analysis consume the same `HirProgram` for normal source.
5. synthetic compiler paths use the same resolver implementation.
6. every analyzed expression has an expression ID and a TypeId fact, including
   nested expressions and `Any` results.
7. recursive/forward type references prefer promoted definition roots over
   conservative expression facts.
8. workspace expression IDs and position queries are deterministic within one
   snapshot.
9. `xl show` exposes expression facts without executing additional XL code.
10. existing runtime, metadata, codec, schema, quota, and CLI behavior remains
    unchanged.

## Implementation plan

1. Extract lexical indexing from `semantic.rs` into a reusable `hir` module.
2. Construct HIR at the start of normal analysis and retain it in `Analysis`.
3. validate compiler source and synthetic programs through the shared resolver.
4. record expression descriptors during existing inference and intern facts.
5. remap HIR and fact IDs during workspace snapshot construction.
6. extend query and CLI observation surfaces.
7. add focused resolution, expression, recursion, and no-re-evaluation tests.

## Deferred work

- partial/recoverable HIR;
- definition-ID-based closure capture and register environments;
- expression facts refined after flow-sensitive analysis;
- field/member semantic identities;
- cached HIR and cross-revision identity reuse;
- serialized query DTOs and LSP.

## Implementation result

Implemented as specified.

`hir.rs` now owns the source-shaped identity layer and the shared lexical
resolver. It assigns deterministic local IDs to definitions, references, and
expressions; represents external and unresolved names explicitly; gives
`decl`/`def` one definition identity; and covers parameters, patterns,
shadowing, recursion, and nested expressions. Normal analysis retains this
`HirProgram`, while metadata initialization and isolated tool expressions run
the same resolver over their synthetic AST. Compiler register and capture maps
remain name-keyed local implementation details as permitted by this RFC.

Analysis now records both definition and expression `TypeId` facts. The
existing descriptor inference walk was made observable rather than replaced:
it records literals, operands, callees, arguments, conditions, match inputs,
interpolation expressions, annotations, and nested block expressions without
performing additional XL evaluation. Promoted type roots replace conservative
bootstrap facts for recursive and forward type definitions.

`WorkspaceSnapshot` projects the retained HIR into workspace definition,
reference, and expression identities. The former `SemanticIndexer` and its
independent scope implementation were removed. The query surface now includes
`expressions`, `expression`, `expression_at`, and `type_of_expression`;
`type_at` prefers resolved definition facts before the narrowest expression
fact. `xl show` lists expression facts, and its position form reports the
narrowest expression and type alongside definition and reference information.

Tests cover shared resolution for declarations and definitions, recursion,
shadowing, parameters, patterns, and externals; fact completeness for every HIR
expression; mixed XL/JSON workspace projection; recursive promoted type roots;
position queries; and CLI observation. The complete workspace suite passes
with 149 unit tests, one ignored manual parser baseline, and six CLI tests.
Strict formatting, Clippy with warnings denied, and diff checks also pass.

Type descriptors are currently interned while each expression fact is
materialized. Structurally equal composite descriptors may therefore occupy
distinct graph nodes and make diagnostic `show` output larger than necessary.
Structural graph interning is deferred as a performance and presentation
improvement; it does not change fact identity or query semantics.
