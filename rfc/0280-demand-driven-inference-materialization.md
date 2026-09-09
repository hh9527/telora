# RFC 0280: Demand-Driven Inference Type Materialization

- Status: Draft; initial investigation complete, implementation not started.
- Related: RFC 0276, RFC 0277, RFC 0279; performance observations in #173.
- Investigation baseline: `13d5859` (2026-09-09).
- Scope: internal inference queries and descriptor materialization shared by
  ordinary analysis and tool inference. No language or public API change.

## Motivation

RFC 0276 established POD inference nodes, constructor rows, shared variable
slots, and direct structural/nominal publication. The remaining inference
consumers still frequently materialize recursive TypeDescriptor trees to ask
questions about a constructor, a selected child, or unresolved variables.
Those temporary descriptors may then be imported back into the slot graph.

The objective is to reduce the demand for complete normalization, rather than
merely add a broader cache around the current owned-descriptor API.

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
| `metadata.rs`, infer_tool_expression_evidence | Materializes each recorded slot into an owned descriptor at evidence export | Audit consumers, share read-only materialization where useful, retain complete evidence |

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

## Design

### 1. Separate inspection, graph predicates, and materialization

Use three distinct operations with explicit contracts:

- **Head inspection** follows proxies and, where the caller's existing semantics
  require it, named aliases. It exposes the constructor and child references
  without recursively resolving the children.
- **Graph predicates** traverse only the necessary edges: for example function
  result chains for expects_type_value, or reachable unresolved slots for
  contains_type_variable. They return facts, not reconstructed type trees.
- **Materialization** produces the fully normalized descriptor expected by a
  genuine descriptor consumer, such as diagnostics or a remaining evidence API.

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

### 3. Share materialization only across a valid read-only boundary

After the relevant constraints and interpolation obligations finish, a
read-only materialization session may memoize by resolved slot/type-row identity.
The session is tied to one solver and one fixed inference/context state; it
must not survive a mutation, a changed lexical instantiation, or context refresh.
Snapshot entries may contain unresolved references if the existing consumer
permits them; publication validation still rejects invalid public types.

Shared descriptors should be consumed by reference or Arc where compatible.
Wrapping a cached tree in Arc and then deep-cloning it into every owned
HashMap entry is not sufficient. Audit evidence consumers before changing
their ownership API; preserve constructor owners, call/trait evidence,
interpolation evidence, lexical scopes and provenance. The exported descriptor
must not retain an arena-local slot after the solver is destroyed.

The first stages do not change global revision invalidation. Dependency-scoped
cache invalidation or permanent stable-body caching is conditional on measured
benefit and a proof covering proxy changes, known-slot updates, conflicts,
nominal body completion and metadata context changes. No global cache keyed
only by source location, Arc address or a nominal constructor is introduced.

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

## Non-Goals and Deferred Alternatives

- Changing eager partial analysis, builtin initialization, module caching or
  Program cloning. They remain separate workstreams from #173.
- Replacing the POD solver, introducing public union/Any, or copying branch
  environments. This is a consumer migration around the existing solver.
- A single global solver spanning modules/tool phases, which requires additional
  dependency and failure-isolation design.
- Eagerly normalizing the entire arena after every constraint or deduplicating
  different generic instances because their current descriptors compare equal.
- Removing all intermediate materialization in one change. Diagnostics, patterns,
  schemes and specialized semantic adapters need individually audited boundaries.

## Implementation Plan

Commit the accepted RFC before implementation, following the repository RFC
workflow. Each stage has a reviewable diff and independent measurements.

0. Extend `scripts/measure-tool-inference.py` to reproduce plain/property cases
   and shared wide/deep type consumers. Capture a preserved baseline binary.
   Use opt-in development counters for root normalization calls, visited nodes,
   descriptor views, cache lookups/misses/invalidations, and caller categories.
   Keep counters out of production behavior and use uninstrumented binaries for
   timing comparisons. Distinguish repeated calls from allocation stack counts.
1. Remove Struct/Enum clone-and-replace construction. Add head-query support and
   migrate the Unchecked fast rejection plus selected small predicates. Leave
   unproven semantic cases on the existing fallback; measure each change.
2. Add graph predicates and migrate selected call/contextualization consumers.
   Retain slot edges across constraint propagation; avoid normalizing whole
   function signatures merely to inspect their head or one parameter.
3. If post-migration profiles still show repeated boundary materialization,
   introduce a read-only materialization session and migrate evidence consumers
   that can share its results. Preserve direct graph publication. If the cost
   is no longer material, record that evidence and explicitly defer this stage.

## Acceptance

Correctness checks must cover observable semantics and graph invariants:

- Differential graph-query results against existing normalization for primitives,
  functions, wide records, nominal arguments/bodies, Bound parameters, aliases,
  Never, pending alternatives and Unchecked. Fallbacks count as correct behavior.
- Query before and after Unknown binding, proxy merge, known-slot change and
  propagated conflict. Preserve recursive body completeness and generic isolation.
- Deep iterative queries and shared DAGs; supported head queries must not create
  descendant descriptor views or allocate proportional to total reachable depth.
- Tool-stage constructor/property/trait/interpolation cases, `.type` diagnostics,
  runtime owners, provenance, checked construction and failing evidence remain
  correct. A failed query cannot authorize publication of incomplete evidence.
- `cargo test --workspace`, release build, `git diff --check` and source-size
  checks pass. Report existing formatting differences without unrelated churn.

Performance evidence must include before/after uninstrumented release binaries,
identical successful inputs, one warmup and at least five samples per case:

- Constant/small-module control; plain/property types at 100/200/400 and a larger
  scale if resource limits permit; shared wide/deep types and generic calls;
  the repository codec-schema case. Do not compare success with failure paths.
- Wall-time distribution, CPU samples, total allocation calls and peak heap;
  allocation bytes when available. Inclusive stacks must not be added together.
- Demonstrate lower normalization/materialization work on at least one targeted
  shared-type workload, without a repeatable material wall-time or peak-memory
  regression on controls. If results are within noise, report no proven speedup
  and improve measurement before claiming success.
- Cache stages require actual hit/miss and invalidation evidence. No target
  percentage improvement is asserted by this draft.
