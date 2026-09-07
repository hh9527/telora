# RFC 0042: Recoverable HIR and semantic fact states

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Recovery and incomplete type evidence have no Any representation or executable authority.
- Depends on: RFC 0008, RFC 0038, RFC 0041

## Summary

XL makes semantic snapshot construction tolerant of incomplete source without
turning syntax errors into runtime language values. Lossless CST and typed CST
queries remain the authority for damaged syntax. Best-effort lowering retains
complete semantic subtrees, and resolved HIR records every definition,
reference, and expression whose required shape is known.

Semantic results use an explicit fact state rather than conflating editing
uncertainty, contradictory evidence, execution limits, and the XL type `Any`:

```rust
struct SemanticFact<T> {
    value: Option<T>,
    state: FactState,
}

enum FactState {
    Known,
    Unknown(UnknownReason),
    Conflicted(Conflict),
    Incomputable(IncomputableReason),
}
```

`value` is the best trustworthy approximation currently available. It may be
present for a conflicted or incomputable fact. Diagnostics and causes remain
structured records owned by the snapshot rather than strings embedded in type
metadata.

This RFC establishes recovery and the status vocabulary. Dependency-scoped
continuation of failed tool-stage evaluation is a subsequent RFC.

## Motivation

RFC 0041 gives complete valid programs one shared identity and expression-fact
layer. It deliberately rejects invalid input before AST lowering or analysis.
An editor, however, must answer queries around a damaged region while the user
is typing. Treating the entire module as absent loses definitions and types
that the CST already represents reliably.

Using `Any` as the recovery result would be incorrect. `Any` is ordinary XL
TypeMetadata and means that the program intentionally accepts lost precision.
It cannot explain whether tooling lacks syntax, encountered contradictory
contracts, exceeded quota, or reached an operation unavailable during the tool
stage.

## Recovery boundary

The runtime AST remains executable syntax. `ExprKind` does not gain a
`Missing`, `Invalid`, or `Error` alternative, and no recovery sentinel can
reach bytecode, a heap, promotion, codecs, or user XL code.

Parsing exposes two related products:

- an optional complete `Program` used by analysis, compilation, and execution;
- a recoverable semantic input made from complete CST subtrees and missing-slot
  records.

Best-effort lowering obeys these rules:

- lower a semantic node only when its required grammar slots are present;
- retain valid sibling bindings and expressions around an error subtree;
- retain zero-width missing-slot locations and invalid-subtree ranges as
  recovery issues;
- never synthesize a source name, expression, binding, or scope edge;
- accumulate diagnostics rather than failing after the first lowering error.

The complete `parse` and compiler APIs remain strict. They return a program
only when syntax and lowering diagnostics are empty.

## Recoverable HIR

HIR construction accepts the best-effort semantic input and produces identities
for every retained node. Missing nodes do not receive expression IDs. A valid
reference may resolve to a retained definition even when another subtree in
the same module is invalid.

Resolution remains explicit:

```rust
enum HirResolution {
    Definition(HirDefinitionId),
    External,
    Unresolved,
}
```

`Unresolved` means that no definition can be established in the current
recovered scope. It does not create a placeholder definition. A missing binding
name likewise creates no definition. Later declarations may still be indexed
when the surrounding block structure is trustworthy.

Recovery issues are associated with source locations and optionally with the
nearest retained HIR identity. They are not HIR expressions and are not
included in expression-fact completeness assertions.

## Semantic fact model

The protocol-independent tooling API defines:

```rust
struct SemanticFact<T> {
    value: Option<T>,
    state: FactState,
    causes: Vec<FactCause>,
    diagnostics: Vec<DiagnosticId>,
}
```

The initial implementation may store one cause and diagnostic internally while
the public semantics remain plural and compositional.

### Known

`Known` means the current snapshot has a complete answer under the implemented
analysis rules. Its value must be present.

`Known(Any)` is valid and distinct from every unavailable state.

### Unknown

`Unknown` means present information is insufficient to select an answer.
Initial reasons include:

- `MissingSyntax`;
- `InvalidSyntax`;
- `UnresolvedName`;
- `BlockedBy(FactIdentity)`;
- `UnavailableDependency`.

Additional information or repaired source may turn an unknown fact into known.

### Conflicted

`Conflicted` means available evidence imposes incompatible requirements. It
retains the conflict causes and may retain a conservative value or candidate
set. Initial conflicts include duplicate definitions and incompatible declared
and inferred contracts.

A conflict is a program diagnostic, not an absence of information. Adding more
unrelated information does not normally resolve it.

### Incomputable

`Incomputable` means the requested semantic computation is well-formed but
cannot be completed in the current tool execution. Initial reasons include:

- `QuotaExceeded`;
- `RuntimeOnly`;
- `UnsupportedOperation`;
- `CyclicEvaluation`;
- `Cancelled`.

Quota exhaustion and cancellation are not type conflicts. An available
declaration may remain as the fact value while its computed refinement is
incomputable.

## Fact propagation

This RFC implements conservative structural propagation only:

- an ordinary complete expression records `Known(inferred_type)`, including
  `Known(Any)`;
- an unresolved variable records `Unknown(UnresolvedName)`;
- a retained parent requiring an unavailable child records `Unknown(BlockedBy)`
  unless it has an independently trustworthy approximation;
- an incompatible annotation and inferred descriptor records `Conflicted`;
- existing strict analysis still reports its current diagnostics and stops;
- no failed tool expression is resumed in this RFC.

Facts never silently change from unavailable to `Known(Any)`.

## Workspace queries

Workspace definitions and expressions expose `SemanticFact<WorkspaceTypeId>`
instead of only `Option<WorkspaceTypeId>`. Convenience type queries continue
to return the fact's optional best value, while new fact queries expose state,
cause, and diagnostics.

Position queries over a damaged source may therefore return:

- a syntax node but no semantic identity;
- a definition or expression with a known type;
- a retained identity with an unavailable type fact;
- explicit absence when no entity covers the position.

`xl show` prints non-known states and their reason. It does not print unknown
facts as `Any`.

## Diagnostics

Parse, lowering, resolution, and fact diagnostics are accumulated in source
order and deduplicated by stable kind and location. A diagnostic may cause more
than one blocked fact, but it is rendered once. Secondary labels survive the
recovery path.

Strict CLI commands (`check` and `run`) keep their fail-fast exit contract for
now. Snapshot and `show` construction expose all available diagnostics and
facts.

## Non-goals

- continuing VM execution after a failed instruction;
- dependency-scoped tool-stage partial evaluation;
- publishing a failed module into Main World;
- recovery sentinels as XL values or TypeMetadata;
- stale facts borrowed from a previous source revision;
- open-document overlays, cancellation transport, or LSP protocol handlers;
- flow-sensitive narrowing, unification, or subtyping.

## Acceptance criteria

1. valid programs retain identical executable AST, HIR resolution, type facts,
   runtime behavior, and diagnostics.
2. the strict parser still rejects any syntax or lowering diagnostic.
3. recoverable parsing retains valid top-level bindings on both sides of a
   damaged binding.
4. recoverable HIR assigns identities only to semantic nodes with required
   slots and resolves references without fabricating definitions.
5. `SemanticFact` distinguishes known `Any`, unknown, conflicted, and
   incomputable states.
6. an unresolved retained reference produces `Unknown(UnresolvedName)`, not
   `Known(Any)`.
7. conflicting contract evidence retains a diagnostic and a `Conflicted`
   state where analysis reaches that fact.
8. workspace position queries can observe retained identities and fact states
   in a module that also has syntax diagnostics.
9. diagnostics are deterministic, source ordered, and not duplicated merely
   because several facts depend on one failure.
10. no recovery node or unavailable fact can enter compilation, VM values,
    heap promotion, codecs, schema generation, or ordinary XL equality.

## Implementation plan

1. define public fact-state, reason, cause, and fact identity records.
2. split strict AST lowering from best-effort semantic lowering over typed CST
   views.
3. extend HIR construction to accept retained semantic nodes plus recovery
   issues.
4. wrap definition and expression type observations in semantic facts.
5. project recovered HIR and facts through `WorkspaceSnapshot`.
6. expose recovery diagnostics and fact states through `xl show`.
7. add malformed-source, known-Any, unresolved, conflict, and runtime isolation
   tests.

## Deferred work

- dependency graph identities for facts;
- continuation of independent tool expressions after one evaluation failure;
- candidate-set presentation for all conflict forms;
- recoverable member and completion facts;
- overlay revisions, cancellation, and LSP transport;
- incremental reuse across snapshots.

## Implementation result

Implemented with a deliberately conservative recovery surface.

`FrontendParse` now always contains a `RecoveredProgram` beside its optional
strict `Program`. The recoverable form is not an executable AST: it contains
only bindings that the existing CST-to-AST lowerer could lower completely and
an optional complete result expression. Each top-level binding is lowered
independently through the same `Lowerer`, so a missing value removes that
binding without removing complete siblings before or after it. Lowering
diagnostics are deduplicated with parser diagnostics and sorted by source
offset. The strict parser still returns `Program` only when the complete
diagnostic set is empty.

`HirProgram::resolve_recovered` runs the RFC 0041 resolver over retained
bindings and an optional result. The resolver's block implementation is shared
between complete and recovered inputs. It creates no identity for a missing
binding and retains ordinary unresolved reference records for complete
expressions.

The workspace query model now exposes `SemanticFact<T>`, `FactState`,
`UnknownReason`, `Conflict`, `IncomputableReason`, `FactIdentity`, and compact
`DiagnosticId`. Definition and expression type observations are facts rather
than bare optional IDs. Complete analysis produces `Known`, including a real
`Known(Any)` for conservatively inferred parameter references. Recovery emits
`Unknown(UnresolvedName)` for unresolved variables and conservative
`Unknown(InvalidSyntax)` for other retained nodes until partial analysis runs.
The four-state protocol and optional best value are public; incomputable
production is intentionally left to the tool-evaluation RFC.

`WorkspaceSnapshot::recover_source` builds a single-source recovery snapshot
without invoking compilation, the VM, heap promotion, codecs, or schema
generation. It exposes accumulated diagnostics, recovered HIR identities, and
unavailable facts. Duplicate recovered definition slots are marked
`Conflicted(DuplicateDefinition)` and share one diagnostic ID. This is the
first concrete conflict producer; incompatible evaluated contracts remain on
the strict analysis path until partial tool evaluation can retain a snapshot
after that failure.

`xl show` selects this recovery path when the root source has syntax
diagnostics and prints diagnostics plus non-known fact states. A valid root
continues through the normal closed-world module loader. `xl check`, `xl run`,
and compilation remain strict.

Tests cover sibling recovery around a missing binding, absence of a fabricated
definition, recovered HIR name resolution, unresolved-name facts, an actual
known `Any`, all four fact states, duplicate-slot conflict diagnostics, CLI
recovery, and strict `check` behavior. The complete suite passes with 153 unit
tests, one ignored manual parser baseline, and seven CLI tests. Formatting,
strict Clippy, and diff checks pass.

Two boundaries remain explicit. Recovery currently descends complete
top-level siblings; it does not yet retain independently complete descendants
inside a binding whose required outer shape failed. Also,
`recover_source` is a single-source tooling constructor: a failed imported
module is not yet projected into an otherwise complete workspace graph. Those
extensions belong with dependency-scoped partial analysis and evaluation,
where blocked facts can carry real cross-node causes rather than guessed
relationships.
