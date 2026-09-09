# RFC 0280: Arena-Based Type Representation Across Inference and Tooling

- Status: Accepted; implementation in progress on `feat/0280-arena-type-consumers`.
- Tracking: [#175](https://github.com/hh9527/telora/issues/175).
- Related: RFC 0276, RFC 0277, RFC 0279; performance observations in #173.
- Profiling baseline: `13d5859` (2026-09-09); arena investigation: `ee04bd9`.
  The intervening updates are documentation changes, not new performance data.
- Scope: migrate internal type consumers and phase boundaries to arena-owned
  graphs with typed integer references. No language or Host behavior change;
  preserve public Rust descriptor interfaces through explicit boundary adapters.

## Revision of the Initial Draft

The initial draft proposed fewer normalization calls followed by an optional
shared-descriptor materialization cache. This revision changes the endpoint:
inference, tool evidence and internal scheme/identity consumers should retain
graph references instead of recursive type trees. A larger tree cache is not
the target architecture. Removing redundant clones remains an early migration
step; graph-backed tool evidence and internal templates are now explicit work.
This revision records the change of direction without claiming implementation.

## Motivation

RFC 0276 established POD inference nodes, constructor rows, shared variable
slots, and direct structural/nominal publication. The remaining inference
consumers still frequently materialize recursive TypeDescriptor trees to ask
questions about a constructor, a selected child, or unresolved variables.
Those temporary descriptors may then be imported back into the slot graph.

The objective is to keep semantic type edges in arenas throughout internal
processing. Solving produces graph facts; normalization resolves/canonicalizes
edges; publication maps graph IDs into the next owner. None inherently requires
building a pointer-linked TypeDescriptor tree. Formatting and serialization can
also traverse graphs directly. Descriptor materialization is a compatibility
boundary for existing APIs, not the default internal exchange format.

## Investigation

### Measured evidence

Release with debug information, rustc 1.98.1; no source instrumentation.
Hardware counters were unavailable. CPU observations use `cpu-clock:u` at
499 Hz with DWARF stacks. Wall measurements use one warmup and five runs,
without a profiler or a concurrent build. Allocation counts use heaptrack.

The synthetic modules contain N independent three-field structs and N simple
functions with explicit Fn(T) -> Int contracts. Both variants declare the same
Label property/provider; only the property variant applies it to all N structs.
All checks succeed. Measurements include process and builtin startup.

| N | Plain, mean +/- standard deviation | With property |
| --- | ---: | ---: |
| 100 | 262.9 +/- 13.6 ms | 278.0 +/- 15.7 ms |
| 200 | 365.6 +/- 6.2 ms | 420.4 +/- 25.0 ms |
| 400 | 617.4 +/- 15.1 ms | 716.1 +/- 25.9 ms |

At N=400, plain/property checks allocated 2,777,025 / 3,104,739 times.
Allocation stacks through normalize accounted for 27.95% / 27.82%; stacks
through tool expression inference accounted for 37.78% / 39.82%.
CPU samples through normalize accounted for 11.14% / 10.79%, and through
tool expression inference for 27.60% / 28.21% (1,221 / 1,400 samples).
These are overlapping inclusive paths, not additive phase budgets.
Allocation counts are not cumulative bytes or live-object counts.

The repository codec-schema check also succeeded in ten sampled runs:
normalize accounted for 11.62% and tool inference for 17.88% of 671 CPU
samples. Its builtin startup share was much larger than in the synthetic cases.
Some stack roots were unavailable and inline expansion was disabled in perf
reports; these percentages are approximate observations, not speedup promises.

Raw local observations and inputs are in `/tmp/telora-perf-173/README.md`.
They are temporary artifacts, not a prerequisite for future acceptance; phase
0 below makes the workload generation reproducible in the repository.
No cache hit rate, invalidation count, normalization invocation count, or
implementation speedup has yet been measured.

### Code findings

Paths below are relative to `crates/telora-core/src/types/` at the baseline.

| Path | Current behavior | Feasible change |
| --- | --- | --- |
| `inference-expression/core.rs`, unchecked_conversion_type | An Inference input can call expose_named, which fully normalizes even when the constructor is not Unchecked | Inspect the constructor first; retain the semantic conversion path only for candidates |
| `inference-expression/core.rs`, call inference | Normalizes the entire Function and repeatedly normalizes parameters for predicates | Read parameter/result edges and query only the required reachable graph |
| `inference-expression/core.rs`, contextualize_authored_literal | Normalizes the recorded type before inspecting the authored constructor | Inspect the slot, preserve its edges, materialize only for consumers still requiring a tree |
| `inference-unify.rs`, numeric/ordered requirements | Fully normalizes before matching a small set of accepted heads | Inspect head; retain full formatting only on the diagnostic path |
| `inference-context.rs`, normalize Struct/Enum arms | Clones the whole field map, then normalizes/replaces each cloned value | Build each output key/value once, preserving order and normalization semantics |
| `metadata.rs`, infer_tool_expression_evidence | Materializes each recorded slot into an owned descriptor at evidence export | Publish an owned graph and ID records; migrate consumers while retaining complete evidence |

Existing `InferenceVariables::head` can bridge authored descriptors and slots,
but resolving a slot through it may allocate a descriptor_view. It is shallow,
not a universal allocation-free graph query. Direct `known`, `constructor` and
`arguments` access already exists and is the preferred basis for slot queries.

The normalized-body cache is narrower than a general normalization cache:
normalize_body looks up Arc addresses registered by descriptor_view, then uses
an arena-revision-tagged entry. Other structural normalization still builds
owned trees. Fresh nodes need not advance revision, while bindings, proxy
changes and conflicts do. Current tests also allow a known slot to be updated;
absence of Unknown is not proof that a slot is permanently immutable.

Ordinary structural and nominal publication already traverses slots directly
in inference-publication.rs. This RFC preserves and builds on that work; it
does not propose replacing it with a new descriptor-based publication pass.

### Arena and ownership inventory

The following was checked against `ee04bd9`; paths are relative to
`crates/telora-core/src/`.

| Representation | Owner and current structure | Remaining boundary |
| --- | --- | --- |
| `InferenceVariableId` / `InferenceTypeId` in `types/inference-variables.rs` and `inference-table.rs` | One solver; POD state nodes, constructor rows and flat slot argument edges | Most inspection APIs still accept/return descriptors; descriptor_view creates shallow pointer-owned views |
| `AnalysisTypeId` / `TypeGraph` in `types/graph.rs` | Analysis graph; Vec of TypeNode, child IDs including function/field payloads | Struct/Enum maps and argument Vecs allocate per node; DeclaredTypeId still embeds recursive arguments |
| `TypeId` / `TypeStore` in `type_store.rs` | Shared runtime store; interned TypeShape with child TypeIds, reserve/seal/abort lifecycle | Variable-length payloads still have per-shape allocation; graph canonicalization imports nominal arguments via descriptors |
| `ToolExpressionEvidence` in `types/tool.rs` and `metadata.rs` | Expression preparation result; owned descriptors per location and runtime type name | Solver is dropped after every record is normalized; arity and owner consumers then inspect these trees |
| `TypeScheme` / `ModuleInterface` in `types/descriptor.rs` | Exported templates and module contracts | Scheme bodies/concrete types are descriptors; qualification recursively renames/copies them |
| `DeclaredTypeId` in `value.rs` | Nominal constructor plus arguments | Stores both Arc<[TypeDescriptor]> and Arc<[TypeExprId]>; applied/reapply builds recursive identity terms as well as descriptors |
| `TypeExprId` in `types/descriptor.rs` | Symbolic identity term | Despite its name, it is a recursive enum with boxed children, not an arena integer ID |

Three domains already exist; replacing them with one global u32 would lose
ownership and semantic distinctions. Analysis graphs can contain Bound/template
nodes; runtime canonicalization rejects Bound and unresolved Named/Pending nodes.
Runtime TypeId also encodes Unchecked in its high bit and is interpreted by its
own TypeStore. These are not interchangeable indices.

The normal compiler already obtains function arity from Analysis.expression_types
and TypeGraph::node (`compiler/frontend.rs`). In contrast, the tool compiler path
normalizes expression records into descriptors before extracting arities and
selected nominal owners (`types/metadata.rs`). This provides a concrete
migration precedent.
Pattern analysis still accepts descriptors (`pattern.rs`), and trait/scheme
consumers need a separate adapter audit rather than an indiscriminate API switch.

Runtime Type metadata remains language data, with heap ownership, lexical type
witnesses, property records and source provenance. Heap::type_descriptor_value
currently bridges descriptors into that representation (`heap/value-builder.rs`).
A canonical TypeId alone does not replace those values or their evidence.

## Design

### 0. Target data flow and lifetime rules

```text
owned module/template graph --instantiate/import--> mutable inference arena
                                                   |
                                  solve + validate + graph-to-graph publish
                                                   |
                               owned analysis/tool type graph + evidence IDs
                                                   |
                                  canonicalize concrete reachable subgraph
                                                   |
                                   runtime TypeStore + separate metadata values
```

Reuse the existing inference arena, TypeGraph and TypeStore. Tool evidence owns
or shares the analysis-style graph it references; it must not retain bare IDs
into a destroyed GenericInference. Prefer publishing reachable evidence roots
into an owned graph to retaining an entire solver and its scratch state.
Generic evidence may retain valid Bound nodes with their lexical scope mapping;
it must not be forced into a concrete runtime TypeId prematurely.

Each conversion uses a memoized source-ID to destination-ID map scoped to the
source owner and destination session. Reserve nominal destinations before
descending into recursive bodies, then validate/seal them; failures cannot expose
Pending/Conflicted nodes. Repeated roots in that session reuse the same mapping.
Do not create a new whole-graph conversion cache for each expression record.

Typed wrappers keep inference slots, constructor rows, analysis IDs and runtime
IDs distinct. Equal integers from different graphs/stores do not establish type
equality. Cross-owner queries import/remap into a known owner or explicitly carry
both owners; no raw-index comparison or hash is a cross-module identity proof.
Do not conflate structural equality with equality of inference variables.

Type edges become IDs; strings, source locations, property values and diagnostic
labels remain separate payloads. An Arc for ownership of an entire immutable
graph is compatible with this design; a recursive Arc tree per type is not its
internal exchange format. Names may use interned symbol IDs, but semantic nominal
identity continues to use declaration/constructor identity rather than spelling.

Logical arena migration comes first. TypeGraph/TypeStore already have ID child
edges; their ordinary structural Vec/Box/BTreeMap payloads do not recursively
own child type trees (nominal argument keys are the exception audited above).
Subsequently, measured hot variable-length payloads can use continuous
argument/field tables with offset/length ranges, following the inference table.
Preserve deterministic field order and lookup behavior. Do not break the public
TypeNode/descriptor view API solely to pack storage; isolate internal storage
behind accessors and compatibility views if such packing is warranted.

### 1. Separate inspection, graph predicates, and materialization

Use distinct operations with explicit contracts:

- **Head inspection** follows proxies and, where the caller's existing semantics
  require it, named aliases. It exposes the constructor and child references
  without recursively resolving the children.
- **Graph predicates** traverse only the necessary edges: for example function
  result chains for expects_type_value, or reachable unresolved slots for
  contains_type_variable. They return facts, not reconstructed type trees.
- **Publication** maps validated graph roots into another owned graph/store.
- **Materialization** produces a descriptor only for an explicitly retained
  compatibility API. Diagnostic formatting should migrate to graph traversal
  rather than require materialization merely to produce text.

A query may operate on an arena slot or a borrowed authored descriptor. A small
internal cursor/view can represent that distinction without importing an entire
authored type merely to inspect it. API spelling is an implementation choice.
Slot IDs and constructor IDs remain arena-local, distinct from final TypeIds.

Head inspection must distinguish Unknown, Known and Conflicted. It must not
interpret `known(slot) == None` as sufficient proof of Unknown. Named alias
cycles and incomplete nominal body views preserve their existing behavior.
Acquiring child IDs before mutating inference is allowed; copying O(arity)
integer IDs does not require cloning their reachable types or an environment.

The two unresolved-variable predicates have different semantics:
contains_type_variable traverses nominal arguments AND bodies;
contains_exposed_type_variable traverses nominal arguments but NOT bodies.
Bound parameters remain rigid parameters, not unresolved inference variables.
Keep this distinction in graph queries and tests.

PendingAlternatives and Unchecked can change semantic shape during normalization.
Do not replace predicates on their normalized result with raw constructor tests.
Initially retain an explicit normalization fallback for these cases and any
unproven named/nominal completion path. Generalize only with equivalence tests.

Queries should use iterative traversal for potentially deep graphs, with cycle
guards and shared-node visitation. Avoid clearing an array for the entire arena
on every small query; use bounded/reusable scratch state where measurements
justify it. A fact observed before a solver mutation must be queried again if
that mutation can affect the answer.

### 2. Reduce redundant construction inside the compatibility adapter

Retain normalize as a compatibility operation during migration. Construct
Struct/Enum output maps directly from normalized entries rather than cloning
all values and replacing them. Preserve deterministic key ordering, tag names,
nominal identity arguments, recursive-body completeness and error rendering.
Do not add an unconditional full-tree "already concrete" scan before every
normalization: that can double traversal while still requiring an owned clone.

The first migrated call sites are the Unchecked rejection guard, simple
head predicates and selected expression-context checks. Function-call migration
follows separately because bidirectional constraints can refine parameter slots
while arguments are processed. Preserve argument checking order and separate
generic instantiation slots; equal resolved types do not imply shared constraints.

### 3. Publish graph-backed tool evidence

Replace the internal all-expression descriptor table with an owned graph plus
location-to-analysis-ID records. Runtime type roots, generic scope mappings and
nominal owner roots refer to that graph. Function arity reads constructor edges,
as the normal compiler already does. Keep value constructors, call dictionaries,
trait selection, interpolation and provenance as separate evidence tables.

Publish after the applicable constraints and interpolation obligations finish;
normalize special nodes into their semantic graph form without prematurely
requiring concrete runtime IDs for Bound templates. A migrated publication path
must not normalize every record into a tree and then intern those trees again.
During migration, explicitly listed unsupported forms may use the old adapter.

The runtime metadata bridge consumes graph roots and builds the required values
and witnesses, preserving checked construction, properties and heap provenance.
It shares conversion work within its valid graph/store/heap context. Do not use
a TypeId-only cache for context-sensitive property records or metadata values.
Retain compatibility conversion for external descriptor callers while migrating
internal callers to graph roots.

### 4. Represent templates and nominal arguments as graph references

Internal scheme bodies and module concrete-type records should reference an
owned immutable template/analysis graph. Bound parameter IDs retain binder/scope
ownership; separate calls instantiate fresh variable slots while sharing the
template. Qualifying imports changes name lookup/display data, not nominal
constructor identity or every nested type node.

Replace internal nominal applied keys with constructor identity plus argument
references into the owning canonical symbolic graph. Concrete runtime keys
already use constructor plus TypeId arguments in TypeStore; retain that model.
Until canonicalization, symbolic arguments can contain Bound or inference slots.
Do not intern a supposedly stable key using raw mutable slots: resolve/rekey it
at the appropriate solver boundary or use a dependency-aware transient key.
Equivalent arguments from different owners must first be remapped/canonicalized.
Mutable states cannot become immutable module-template exports.

The goal is to remove internal recursive TypeExprId/TypeDescriptor duplication,
not rename TypeExprId while retaining its boxed tree. Public TypeScheme,
ModuleInterface and DeclaredTypeId adapters may remain, but their trees must not
remain the authoritative representation repeatedly cloned by internal consumers.
Audit trait/property constraint payloads, pattern analysis and runtime owner
construction as part of this migration. Preserve public behavior and parameter,
declaration and recursive-body identity. The new private representation must be
introduced without silently changing the existing public Rust field contracts.

### 5. Bound the remaining adapters and caches

A read-only descriptor adapter can memoize where unavoidable, but it belongs to
one fixed solver/graph/context state. It is not an alternative to graph-backed
evidence. Avoid deep-cloning an Arc-cached tree into every output record, retaining
both representations for all nodes, or keeping an entire solver alive for a few
published roots. Release conversion maps and scratch storage after their owner
session; immutable graphs can be shared across their legitimate consumers.

The first stages do not change global revision invalidation. Any later finer
invalidation must be justified by measured residual cost and cover proxy changes,
known-slot updates, conflicts and context changes. A known head, or absence of
Unknown today, is not a guarantee of permanent stability. Track remaining
descriptor adapters explicitly until their consumers are migrated.

## Semantic Invariants

- Never remains bottom for directional checking; a raw Unknown is not Never.
- Unknown/proxy chains observe later solutions and conflicts. No query freezes
  an incomplete type or publishes Conflicted as a valid type.
- Generic calls instantiate independent slots; lexical shadows remain distinct.
- Nominal identity includes applicable arguments. A recursive stub and a complete
  body are not interchangeable just because their declaration heads match.
- Unchecked conversion, alternative collapse, enum ownership, constructor
  contextualization and `.type` boundaries are unchanged.
- Static Property/trait evidence does not require evaluating provider values;
  provider execution, checks and diagnostics retain their existing order.
- Tool inference still supplies complete evidence. This is not permission to
  skip checking because a descriptor table is already available.
- Direct slot-to-TypeGraph publication and validation remain authoritative.
- A graph reference cannot outlive its owner, and an ID from another owner
  cannot be accepted accidentally. Public runtime IDs obey the existing
  TypeStore reservation and Unchecked rules.

## Non-Goals and Deferred Alternatives

- Changing eager partial analysis, builtin initialization, module caching or
  Program cloning. They remain separate workstreams from #173.
- Replacing the POD solver, introducing public union/Any, or copying branch
  environments. This is a consumer migration around the existing solver.
- A single global solver spanning modules/tool phases, which requires additional
  dependency and failure-isolation design.
- Eagerly normalizing the entire arena after every constraint or deduplicating
  different generic instances because their current descriptors compare equal.
- Removing all intermediate materialization in one change, or changing exported
  Rust descriptor field types without compatibility adapters. Internal patterns,
  schemes and specialized semantics are migrated in audited stages.
- Forcing all runtime metadata values into the type arena, eliminating all
  pointer-based payload storage, or packing every table before eliminating
  recursive type-tree boundaries.

## Implementation Plan

Commit this revised RFC before implementing its expanded arena strategy.
Each stage has a reviewable diff and independent measurements. Update current
design documentation as implementation lands; this draft is not a replacement
for the existing language/Host semantics.

0. Extend `scripts/measure-tool-inference.py` to reproduce plain/property cases
   and shared wide/deep type consumers. Capture a preserved baseline binary.
   Use opt-in development counters for root normalization calls, visited nodes,
   descriptor views, cache lookups/misses/invalidations, graph import/export
   counts, nominal argument-key construction and caller categories.
   Keep counters out of production behavior and use uninstrumented binaries for
   timing comparisons. Distinguish repeated calls from allocation stack counts.
1. Remove Struct/Enum clone-and-replace construction. Add head-query support and
   migrate the Unchecked fast rejection plus selected small predicates. Leave
   unproven semantic cases on the existing fallback; measure each change.
2. Add graph predicates and migrate selected call/contextualization consumers.
   Retain slot edges across constraint propagation; avoid normalizing whole
   function signatures merely to inspect their head or one parameter.
3. Publish graph-owned tool expression evidence and migrate arity/owner consumers.
   Add a direct graph-to-runtime-metadata bridge for supported roots. Preserve
   Bound scopes and all non-type evidence; never return dangling solver IDs.
4. Introduce private graph-backed templates and nominal argument keys, migrating
   internal scheme/module-interface, trait/property and pattern consumers in
   separate substeps. Remove recursive argument-key duplication from migrated
   paths and retain explicit public descriptor adapters at ingress/egress.
5. Remove the remaining internal round trips on the selected inference/tool
   paths. Profile table payloads; pack hot argument/field arrays if justified.
   Report any residual tree adapters and their cost/lifetime. Do not call this
   RFC complete after only reducing clones or adding a descriptor cache;
   significant deferrals require an explicit scope revision.

## Acceptance

Correctness checks must cover observable semantics and graph invariants:

- Differential graph-query results against existing normalization for primitives,
  functions, wide records, nominal arguments/bodies, Bound parameters, aliases,
  Never, pending alternatives and Unchecked. Fallbacks count as correct behavior.
- Query before and after Unknown binding, proxy merge, known-slot change and
  propagated conflict. Preserve recursive body completeness and generic isolation.
- Deep iterative queries and shared DAGs; supported head queries must not create
  descendant descriptor views or allocate proportional to total reachable depth.
- Multiple evidence roots sharing a type publish that type once per conversion
  session. Supported tool arity/owner and internal template paths do not
  materialize whole descriptor trees. Add migration guards/counters for these
  boundaries and report all remaining semantic fallbacks.
- Destroy the solver before consuming its published tool evidence. Test graph
  lifetime ownership, overlapping numeric IDs in separate owners, cross-module
  generic templates, independent instantiations and nominal equality after remap.
- Deep and recursive nominal graph import/publication must reserve before body
  traversal, reject invalid public nodes, and avoid duplicating shared subgraphs.
  Formatting and metadata witnesses must match existing observable behavior.
- Tool-stage constructor/property/trait/interpolation cases, `.type` diagnostics,
  runtime owners, provenance, checked construction and failing evidence remain
  correct. A failed query cannot authorize publication of incomplete evidence.
- `cargo test --workspace`, release build, `git diff --check` and source-size
  checks pass. Report existing formatting differences without unrelated churn.
- Follow `guide/TESTING.md`: check success is not behavioral test success;
  should_ok requires explicit assertions, warnings are not assertions, and
  expected failures need successful controls and diagnostic/provenance checks.

Performance evidence must include before/after uninstrumented release binaries,
identical successful inputs, one warmup and at least five samples per case:

- Constant/small-module control; plain/property types at 100/200/400 and a larger
  scale if resource limits permit; shared wide/deep types and generic calls;
  the repository codec-schema case. Do not compare success with failure paths.
- Wall-time distribution, CPU samples, total allocation calls and cumulative
  allocation bytes when available, peak heap and uninstrumented peak RSS.
  Separately measure retained graph/adapter memory after preparation and across
  repeated operations; profiler RSS includes overhead. Inclusive stacks must
  not be added together or interpreted as predicted memory savings.
- Record graph nodes/edges, bytes retained per owner and conversion-map peaks.
  Shared roots should not retain duplicated trees, and a short-lived tool result
  should not pin unnecessary solver state. Compare wide/shared/generic workloads
  against controls before deciding whether packed payload tables are worthwhile.
- Demonstrate lower normalization/materialization work on at least one targeted
  shared-type workload, without a repeatable material wall-time or peak-memory
  regression on controls. If results are within noise, report no proven speedup
  and improve measurement before claiming success.
- Cache stages require actual hit/miss and invalidation evidence. No target
  percentage improvement is asserted by this draft.
