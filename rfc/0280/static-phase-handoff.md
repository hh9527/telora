# Static result handoff audit

Source audit on 2026-09-10, after the nominal-refinement slot checkpoint.
This investigation changes no compiler behavior and makes no new performance
claim. The latest measured binary remains the refinement-slot checkpoint.

## What the two paths actually do

The pure loader in [module/type-check.rs](../../crates/telora-core/src/module/type-check.rs)
has no VM or runtime heap. It discovers modules, recursively solves their static
interfaces, and invokes `check_module_types`. The latter first calls
`solve_partial_types`, then `solve_program_types`.

This is **not evidence of two complete value-inference runs**: inspection of
[partial-solver.rs](../../crates/telora-core/src/types/partial-solver.rs) shows that
its dependency worklist is built from `type_definition_bindings`. Its HIR,
declaration facts, schemes and graph seed the subsequent full program solver.
Deleting this pass would lose declaration/family preparation. The preparation
should become the shared declaration stage, not be removed as duplicate work.

The pure result, `CheckedModuleTypes`, currently keeps only `ModuleInterface`
and `TypeGraph`. `check_module_types` discards the returned expression-publication
map, its HIR and the remaining inference evidence. The static module loader
places this graph in `SemanticModuleInterface`, while setting the module's
`analysis` and `partial` fields to `None`. This result cannot currently replace
the ordinary compiler's `Analysis` input.

The ordinary path calls `analyze_program_with_bindings_observed` from the strict,
recoverable and builtin loaders. It does not first invoke `check_module_types`.
Running the pure loader in front of the existing ordinary loader would therefore
add a second full program solve rather than reuse a completed static artifact.

## Concrete boundaries still carrying runtime dependencies

| Current site | Evidence | Required destination |
| --- | --- | --- |
| `types/dependency.rs`, analysis setup | Constructs `ToolEvaluator`, installs/reads runtime bootstrap and stages construction checks before `solve_program_types` | Deferred execution/linking, after static completion |
| `types/metadata.rs`, `imported_static_descriptor` | Calls `infer_value_ref` for missing interfaces and certain empty interfaces; even interface selection can inspect `ValueKind::Module` | Static import/Host contracts, resolved independently of runtime values |
| `types/dependency.rs`, NativeType setup | Decodes an already linked heap value to recover its type | Native descriptor supplied from the static builtin inventory |
| `module/graph.rs`, builtin loading | Already computes `declared_native_types`, but publishes native values and passes roots onward | Pass the existing native descriptors into static solving; link values afterward |
| `types/dependency.rs`, interface/export construction | Uses runtime module-kind checks to classify imported values/namespaces | Static import/export and HIR facts |
| `types/properties.rs`, property preparation | Solves target/provider call expressions through `solve_tool_expression_types` after the main program solve | Plan and solve these generated expressions before the static phase closes |
| `types/tool-plan.rs`, solved binding preparation | Already consumes main-solver evidence and shares its graph publication table | Preserve this path in the static artifact; do not reinfer these bindings |

Moving a value-inspecting helper outside a function named “solver” would not
satisfy the VM-free boundary. Every imported runtime binding needs an explicit
static contract, including native types, selected exports, namespaces and Host
inputs. Missing contracts must be diagnosed; an old value-inference fallback
must not mask an incomplete caller migration.

## The next implementation boundary

Create one owned static module result consumed by both check modes and by the
deferred executor. It needs the existing session module/HIR identities, one type
graph, definition/expression/result roots and schemes, static interface facts,
and compiler evidence (constructors, propagation, trait/call/interpolation and
lexical type evidence). It also needs declaration, construction-check and
property execution plans with solved roots. These are requirements for the
artifact, not a claim that a new structure already exists.

Do not put `PersistentValue`, runtime roots, a VM or an evaluator in this result.
Keep runtime links and materialized metadata as a separate execution result.
Do not rebuild HIR or renumber graph roots when attaching those runtime links.
The current `Analysis` mixes both sets of fields; splitting its ownership and
migrating consumers together is necessary. An unused evidence copy attached to
the pure result would not be a completed handoff.

Implement in dependency order:

1. Make builtin, imported and Host binding contracts available as static inputs
   to all three ordinary loader callers. Cover native types, namespace values,
   selected/re-exported bindings and explicit dynamic inputs; remove value-based
   type inference from that input boundary.
2. Share declaration preparation, program solving and evidence finalization
   between both check modes. Retain the expression publication map and compiler
   evidence in the owned static result. Migrate consumers rather than introducing
   a second optional artifact/fallback path.
3. Complete generated tool/check/property plans before closing static solving.
   Require their executor to accept the solved artifact and runtime links only;
   it must have no inference callback.
4. Have the session driver solve all reachable static module results before
   initializing any Telora module values or metadata. The ordinary per-module
   analyze/evaluate loops cannot remain the driver. Keep session output gating
   and independent diagnostic facts without module rollback.
5. Once both check modes use this pipeline, report times at the actual shared
   phase boundaries. Until then, subtracting their elapsed times remains an
   approximate comparison of different paths, not exact phase attribution.

## Evidence required before claiming this handoff is complete

- A downstream type error prevents earlier imported Telora initializers and
  property providers from executing; statically available module/type facts are
  still retained for diagnostics.
- Native descriptors, aliases and Host contracts are resolved with no heap/VM
  capability in the static API, including missing-contract failure cases.
- Generated property/construction plans are fully typed before execution. An
  execution-only test demonstrates that no solver is available or entered.
- HIR and type IDs observed by static solving are the same IDs consumed by tool
  compilation and runtime compilation; no second full solve or graph remapping.
- Existing normal-check runtime diagnostics, property reduction, traits,
  interpreter contracts and CLI language acceptance continue to pass.
- Measure the unified static and deferred phases, then repeat ordinary ontology
  timing and allocation measurements against the preserved pre-handoff binary.

This handoff does not waive the RFC's remaining shared interface table,
Unknown/Conflicted diagnostic accumulation, descriptor-consumer migration or
full semantic/performance acceptance gates.

## Native input contract checkpoint (2026-09-10)

The builtin loader now supplies each native type as an explicit selected-binding
interface containing its inventory-derived `TypeOf(Opaque(...))` contract.
Ordinary analysis reads this contract through `native_type_contract`; it no
longer decodes the native heap value to obtain a descriptor. Missing contracts
and non-concrete/non-opaque contracts are errors. The now-unused evaluator
`decode_type` method is removed rather than retained as a fallback.

The contract reader accepts only static interfaces. A unit test constructs and
validates native identity without any heap/VM and covers missing/invalid
contracts. The full core and CLI language suites exercise builtin linking and
consumption of the new interfaces.

This closes only the native-type *descriptor-source* dependency identified in
the table above. Runtime native values are still linked in the existing builtin
loader, and ordinary analysis still creates an evaluator early. Generic imports
and Host bindings still have the separate value-inference paths listed above.
This checkpoint does not complete VM exclusion for ordinary static analysis
and does not claim a measured performance improvement.

## Import interface contract checkpoint (2026-09-10)

Ordinary `BindingKind::Import` now obtains its type exclusively through
`imported_interface_descriptor`, which accepts no runtime value or heap. The
pure checker uses the same interface rule. `value_binding` distinguishes a
selected value from a namespace; an empty namespace remains an empty structural
namespace, and a selected binding missing its export contract is rejected.
The former empty-interface `ValueKind::Module` test and value-inference branch
are removed. Explicit interfaces in the Host/recovery adapter also follow this
rule instead of inspecting the runtime value to reinterpret an empty interface.

This does not yet remove the Host adapter's no-interface `infer_value_ref` path,
the recovery wrapper's separate imported-value fact projection, or runtime
Module-kind checks elsewhere in interface finalization. Ordinary imports also
still require runtime roots for the existing linking/execution path. Only their
type selection has been detached from those roots in this checkpoint.

The next Host input migration must account for `DataWorld`, which currently
stores a heap and a value but no separate static contract. Primitive factories
know their type at construction; other creation sites need explicit contracts
from their source/codec/solved artifact. Merely publishing a DataWorld and
inferring its value outside the solver would leave the handoff incomplete.

## DataWorld contract checkpoint (2026-09-10)

DataWorld now carries an explicit optional contract. Primitive factories supply
their known type directly. JSON/TOML/YAML parsing derives the contract from the
validated source-data plan before materializing values. This uses an explicit
postorder worklist and canonical TypeGraph rows: shared/forward data edges are
handled, and homogeneous-array checks compare type IDs instead of repeatedly
cloning descendant type trees. A descriptor is produced once at the existing
external interface boundary. Untyped tags and incompatible array element types
remain without a contract; no enum owner or common type is invented for them.

The private DataWorld constructor requires a contract argument. Heap and contract
share one immutable HostData owner, keeping DataWorld cloning independent of
contract size. This is external Host resource ownership, not reference counting
inside the inference arena.

The strict DataWorld analysis wrapper and module dependency preparation pass
these contracts as selected-binding interfaces. Missing contracts fail before
publishing those inputs. The DataWorld partial-analysis wrapper now extracts
static inputs and invokes the pure partial solver directly: it no longer
constructs a heap, publishes inputs, or inspects their values.

Low-level observed/recovery APIs that receive PersistentValue roots directly
still contain the remaining no-interface Host/value-fact adapters. They must be
migrated before deleting the general `imported_static_descriptor` fallback.
This checkpoint does not claim all Host entry points are VM-free or that the
ordinary session driver has reached the final static-first architecture.

## Runtime value inference removed (2026-09-10)

External binding types now come exclusively from static interfaces: selected
bindings, trait dictionary schemes, and hidden property-root contracts. Hidden
property roots use the existing `TypePropertyEvidence.property` contract rather
than inspecting the computed property value. `imported_static_descriptor` and
the recursive `infer_value_ref` implementation have been deleted.

The recovery type-analysis API now takes external names and static interfaces,
not heap, quota, or runtime roots. Missing Host contracts are errors even for
primitive values. Ordinary analysis still creates its evaluator early and uses
runtime roots for linking and some namespace classification; removing value
inference does not yet make that driver VM-free.

The release CLI supports `check --types-only`. Its independent static loader
does not own VM/heap resources and checks tool/function bodies without executing
providers, checks, or module values. Until both drivers share a retained static
artifact, comparing their elapsed times is not exact phase accounting.

## Evaluator acquisition deferred (2026-09-10)

Ordinary analysis now classifies imported/re-exported namespaces solely from
`ModuleInterface.value_binding`, matching the pure checker. Both runtime
Module-kind probes and the now-unused `ValueRef::persistent` adapter are removed.

Evaluator creation, bootstrap installation and pending construction-check staging
now occur after main inference publication and preparation of solved tool,
construction-check and property plans. Bootstrap values are merged before
external links, preserving external shadowing. Static failures return before
this evaluator boundary.

This is a module-local execution boundary, not the final session boundary.
The function still accepts a heap and runtime roots, records links during static
analysis, and the module driver still loads dependencies through the ordinary
execution path. The next extraction must carry static HIR/graph/evidence/plans
across the boundary and defer execution for the entire reachable module graph.

## External execution links deferred (2026-09-10)

The static traversals no longer populate `tool_values` or extract runtime values
from PersistentValue roots. One `link_external_tool_values` step runs after
solved plans are prepared, before evaluator acquisition. It validates dynamic
inputs and import/native links, preserves authored-binding shadowing, and
registers each authored import/native binding once. Missing runtime links are
execution-link errors; static import/native type contracts remain mandatory.

The remaining pre-boundary uses of `external_roots` inspect only names. They
must become an explicit names input when extracting the static solver; the
current function signature still grants access to execution resources.

Further audit: final interface/evidence assembly still occurs after tool
execution and retains the inference object, including normalization of runtime
type evidence and exported schemes. `TypeFamilyTemplate` currently includes
runtime template/root handles produced by declaration materialization. Therefore
simply splitting the function at evaluator creation would be insufficient:
static family contracts and compiler evidence must be finalized separately from
runtime family links, before a self-contained static artifact can be returned.

## Owner evidence prepared before execution (2026-09-10)

Declared-owner lexical evidence selection and parameter substitution now run
before runtime linking/evaluator acquisition. `prepare_declared_value_owners`
accepts syntax, static schemes, inference and the type graph, with no heap/VM.
It produces final compiler owner evidence plus plans containing a link name,
graph root ID and family arity. Runtime materialization consumes these roots in
one batch through the existing graph materialization table; it no longer scans
inference scopes or rebuilds descriptor trees for these owners.

Runtime type-evidence descriptors are also normalized before execution. This
does not finish the handoff: exported interface assembly still reads inference
after execution, and runtime type-evidence materialization still consumes final
descriptors. The existing lexical scope scan and unresolved-owner omission rules
are unchanged; improving those requires a separate correctness/coverage audit.

## Static family interface and solver release (2026-09-10)

The consumer audit found that imported family templates were only propagated
through module interfaces; the actual non-propagating consumer needed the
nominal constructor identity (`std/entry` wrapper validation). ModuleInterface
now carries `type_family_constructors`, containing static constructor IDs/names,
instead of runtime TypeFamilyTemplate handles. Import selection, open imports,
namespace qualification and re-exports propagate the static identities.

Local constructor identities come from solved DeclarationPlans before any
materialization. Runtime family handles remain in Analysis.type_family_values
for compilation/linking; the duplicate exported runtime-template table and
unused copied parameter lists are removed. There is no fallback to inspecting
runtime templates for interface construction.

Ordinary interface construction, exported-scheme validation, property-presence
publication and compiler-evidence finalization now all precede execution. The
driver explicitly drops GenericInference before runtime linking and evaluator
creation. Runtime code below that boundary cannot query this solver or normalize
through it. Property value materialization still verifies that every static
presence record has a value, without changing the static interface.

This closes the previously identified post-execution inference reads in the
ordinary analysis function. It does not complete session-wide ordering or the
VM-free static API: the encompassing function still accepts runtime resources,
and the module driver still executes dependencies while loading them. The pure
loader's static artifact and the ordinary driver's retained evidence also still
need to converge.

## Separate static solver and execution entry points (2026-09-10)

Ordinary analysis now calls `solve_module_plan` followed by
`execute_module_plan`. The solver receives only source syntax, external names,
static interfaces/provenance, query context and TypeStore. Its signature has no
VM, heap, PersistentValue roots, debug executor or runtime quota account.
Source registration stays in the outer driver; static queries receive the
existing query context directly.

`SolvedModulePlan` owns the solved TypeGraph, HIR, interface, compiler evidence
and prepared declaration/property/check/owner plans. Plans borrow the original
source syntax; the artifact does not borrow an inference solver or execution
heap. Graph ownership moves into execution, preserving the published IDs.
SolvedToolBinding is separate from ToolBindingTask, so execution value slots
are allocated only when entering execution. Static names are projected once
from the existing driver's external roots; this bridge is not runtime type
inference.

A direct solver test constructs no heap and solves an exported nominal generic
family together with `1 / 0`, checking static contracts and retained expression
IDs/plans without evaluating that expression. The ordinary driver is still
module-at-a-time; this API extraction does not yet unify the pure loader or
defer every dependency's execution until the complete session graph is solved.

## Types-only uses the same static solver (2026-09-10)

`check_module_types` now adapts native static contracts and invokes
`solve_module_plan`, the same solver used by ordinary analysis. The previous
separate type-declaration/value/decorator checking implementation is removed,
including its now-unused static-decorator checker. No fallback to the old
implementation remains. Existing partial-analysis APIs retain their separate
recovery solver; they are not used as a preliminary full check here.

The static workspace owns a shared TypeStore. Its interface map is moved into
the adapter instead of cloned, and native inventory facts become selected-binding
contracts without constructing runtime values. Types-only consumes the resulting
graph/interface and drops execution plans. It therefore prepares the same tool
plans as ordinary analysis, although it never executes them.

The two modes still have different module-loading/publication drivers, so their
elapsed-time difference is not precise phase accounting. Session-wide retained
plans and deferred ordinary execution remain the next driver migration.

## Tool bytecode generation deferred (2026-09-10)

Tool expression preparation now returns a PreparedExternalExpression containing
the lowered expression and solved constructor/owner evidence, plus its validated
sorted external links. Static preparation still performs type checking,
elaboration and unresolved-name validation, but does not invoke Compiler or
generate LIR/bytecode.

The execution consumer invokes compile_prepared_external_expression on first
use, retaining the resulting bytecode (or compilation error) on the plan for
subsequent uses. This code-generation API takes only prepared syntax/evidence,
external links and source information; it cannot query an inference solver.
Runtime witness selection continues to use the statically prepared link list.

The binding-task regression test checks that preparation leaves bytecode absent,
execution creates it, and repeated direct execution reuses the generated result.
This is not removal of all static tool preparation costs: source lowering,
evidence projection and owner planning still happen before execution. Prepared
syntax is currently retained alongside generated code, pending the broader
arena/typed-IR consumer migration.

## CPU sampling and repeated nominal-body ingress (2026-09-10)

Software perf events work on this machine (`task-clock:u`, `cpu-clock:u`), even
though hardware cycles/instructions are unavailable. An initial 326-sample
types-only recording attributes about 18.7% self time to contains_type_variable,
with callers including nominal body import and normalization. This is a short
profile used to choose an investigation target, not a precise speedup estimate.

`import_declared_body` now checks its existing immutable-body-to-slot mapping
before recursively inspecting the descriptor. Previously the same mapping was
checked only inside import_body, after eligibility scans. A hit reuses existing
arena edges, including unresolved slots; it does not snapshot their solved state.
The existing retained Arc key prevents address reuse. First imports and distinct
recursive views still follow the prior identity/completeness checks.

The post-solving binding publication passes also repeated normalization for
initial ID reservation, validation, interface descriptors and final graph IDs.
They now retain the first normalized binding projection and move it into the
interface map; subsequent consumers reuse it. There is no solver mutation
between these uses, and unresolved/owner validation is retained.

## Data modules contribute contracts only (2026-09-10)

The types-only loader no longer reads or parses JSON/TOML/YAML contents, creates
data source records, or enforces data limits. Each resolved data module contributes
the fixed `{ data: Value }` interface, including a data module selected as root.
Content validation stays in ordinary loading. Static use of `data` still has to
match Value's nominal type; this does not weaken type checking of its consumers.

Core coverage includes invalid UTF-8 data and absence of a data source record.
CLI coverage checks invalid and valid content for all three formats, direct data
roots and imports, and rejection of `data` where Int is required. Full workspace
tests pass (414 core and 45 CLI integration tests, including language acceptance).
No timing or memory gain is claimed for this checkpoint without measurement.

The main-world skeleton/TypeId handoff is the target, not completed by this change.
The loader still obtains Value through per-module builtin solving. The controlling
migration plan is now [whole-graph-ir.md](whole-graph-ir.md); retaining independently
solved module plans is only transitional and cannot satisfy the session IR gate.

## Reachable HIR prepared before module solving (2026-09-10)

The types-only driver now prepares all reachable HIR before invoking any module
type solver. `StaticNames` follows source declaration roles, aliases, namespaces
and selected imports/re-exports without solved ModuleInterfaces. Bootstrap names
have a separate name-only inventory checked against their type contracts. The
classification index is dropped after preparation, and HIR moves into solving
without a rebuild. Unreachable modules do not get HIR from this pass.

The regression test resolves a re-exported lowercase enum constructor pattern
and a same-name local binding while a dependency contains a type error; complete
checking still rejects the error and accepts the corrected program. Core tests
(416), CLI tests (45 including language acceptance), release build, diff and size
checks pass. Additional types-only checks cover prelude constructors and cross-module
construction boundaries. The pre-existing non-prelude open-import failure is
recorded in the whole-graph audit.

This removes one ordering dependency, not module-local type ownership. The
ordinary analyzer still obtains HIR name facts from external interfaces. Stable
session declaration IDs, direct import reference edges, preallocated syntax type
slots, whole-graph constraint solving and finalized typed-IR consumers remain.

Ontology query measurements against the immediately preceding data-contract
checkpoint (`/tmp/telora-session-hir.VQOnos/before`), 2 warmups and 5 runs:

| Mode | Forward old → new | Reverse old → new |
| --- | --- | --- |
| types-only | 708.5 ± 13.2 → 722.8 ± 25.7 ms | 715.0 ± 12.2 → 726.4 ± 23.3 ms |
| ordinary | 943.8 ± 49.0 → 909.9 ± 18.4 ms | 917.0 ± 46.1 → 900.8 ± 21.3 ms |

Types-only means increased by approximately 1.6–2.0%; the small sample and spread
do not support a precise cost estimate, but there is no speedup. Ordinary results
are too noisy to claim improvement. This architectural checkpoint does not meet
the final performance gate by itself.

Types-only heaptrack allocation calls: 2,262,644 → 2,263,129 (+485); peak heap:
83.27 → 83.28 MB (essentially unchanged). Do not use heaptrack elapsed time or RSS
as normal runtime/heap measurements. Artifacts: `/tmp/session-hir-perf{,-reverse}.json`,
`/tmp/session-hir-{before,after}-summary.log`, `/tmp/session-hir-{core,cli,release}.log`.

## Static imports share resolved export-row targets (2026-09-10)

HIR preparation and type input selection now consume one `ResolvedStaticModule`.
Selected imports refer to `(ModuleId, export row)` and namespace imports to
ModuleId. These are source-allocated identities available before type solving;
aliases of the same export share a target. Data modules provide the synthetic
data export row without content parsing. Re-exports still have their own public
rows; connecting them to canonical definition and inference slots remains work.

The static resolver now expands non-prelude open imports, preserving explicit
binding precedence, implicit-prelude fallback, provider deduplication, and
diagnostics only for referenced ambiguous names. Bare constructor patterns and
local shadowing use the prepared HIR; there is no HIR rebuild per ambiguous name.
The previously failing enum-constructors types-only fixture now succeeds.

`StaticWorkspace::solve` updates its interface table in place and returns only
completion. It no longer clones a whole interface to return or reuse a dependency.
Selections still construct descriptor-based input fragments; this is not yet a
replacement for whole-session type-slot edges or the ordinary loader.

Validation: full workspace tests pass (416 core, 46 CLI integration tests,
including language acceptance), release build, diff/source-size checks. New
ordinary/types-only comparison cases cover open re-exports, used/unused ambiguity,
duplicate providers, explicit and lexical shadowing, prelude precedence and bare
constructor patterns. The alias identity assertion runs before dependency typing.

Ontology query compared with the immediately preceding HIR-preparation release
(`/tmp/telora-resolved-imports.G5HarF/before`), 2 warmups and 5 runs:

| Mode | Forward old → new | Reverse old → new |
| --- | --- | --- |
| types-only | 726.5 ± 5.4 → 734.9 ± 37.3 ms | 719.5 ± 3.7 → 719.9 ± 15.9 ms |
| ordinary | 905.7 ± 8.9 → 910.1 ± 28.8 ms | 933.6 ± 40.1 → 899.9 ± 20.9 ms |

The forward types-only sample contains an outlier. Reverse types-only results
are essentially unchanged; ordinary timing changes direction. No stable time
gain is claimed. The new types-only heaptrack run reports 2,262,752 allocation
calls versus the preceding checkpoint's 2,263,129 (-377); peak heap remains
83.28 MB. This does not satisfy the final performance gate by itself.

Artifacts: `/tmp/resolved-imports-perf{,-reverse}.json`,
`/tmp/resolved-imports-after-summary.log`, `/tmp/session-hir-after-summary.log`,
`/tmp/resolved-imports-{workspace,release}.log`.

## Wildcard imports establish search scopes (2026-09-10)

StaticNames now stores open provider ModuleIds and resolves names when HIR
requests them. Only actual external references create wildcard type inputs;
pattern probes that become local bindings do not. Explicit prelude imports
participate in ambiguity checking. The intrinsic PropertyAttr dependency is an
explicit HIR reference, and module trait/property facts travel independently of
selected exported values. The ordinary loader still uses its existing import
preparation; global definition/type-slot ownership remains outstanding.

Immediate baseline is commit `6b020f1`, preserved at
`/tmp/telora-open-scopes.2aimiu/before`. Ontology query, 2 warmups/5 runs:

| Mode | Baseline → current | Reverse order, baseline → current |
| --- | --- | --- |
| types-only | 737.8 ± 15.6 → 747.0 ± 7.7 ms | 736.1 ± 13.5 → 740.6 ± 17.4 ms |
| ordinary check | 933.2 ± 25.9 → 915.3 ± 6.8 ms | 911.0 ± 14.5 → 922.0 ± 23.0 ms |

No stable time improvement. Types-only allocation calls decrease from 2,262,796
to 2,250,067 (-12,729, about 0.56%); temporary allocations increase from 123,784
to 128,365. Peak heap is effectively unchanged: 83.28 → 83.25 MB.

Full workspace tests pass (417 core, 47 CLI including language acceptance),
release builds, both ontology modes and the types-only enum-constructor fixture
pass. Source-size and diff checks pass with existing size review warnings.
Artifacts: `/tmp/open-scopes-perf{,-reverse}.json`,
`/tmp/open-scopes-{before,after}-summary.log`,
`/tmp/open-scopes-{workspace,release}.log`.
