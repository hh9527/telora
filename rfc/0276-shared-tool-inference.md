# RFC 0276: Shared Tool Inference and Static Property Evidence

- Status: In progress on local branch `perf/shared-tool-inference`; shared-slot
  solver implemented, expression-interface migration and main integration pending.
- Baseline: 58f0b8c, after RFC 0274.
- Related: RFC 0260, RFC 0274, RFC 0275.

## Motivation

RFC 0274 introduced constructor inference before each tool expression. Each
invocation copies module environments, decodes every tool binding and rebuilds
declaration evidence. A debug check of 400 independent struct declarations
increased from 3.08 s before RFC 0274 to 11.46 s. Instrumentation attributed
7.58 s to tool inference, including 362,085 type-decoding attempts.

The module already has an authoritative inference pass. Ordinary value bodies
should be checked there; only dependencies of metadata computations need early
tool evaluation. Type identities and declaration contracts are shared module
facts, not inputs that must be reconstructed for each expression.

## Design

Maintain declaration contracts and bodies as module-level inference inputs.
Publish newly established bindings incrementally. Tool expressions use only
their referenced value inputs, including lexical generic witnesses, while
sharing module contracts and declaration identity. Failed or incomplete local
inference must not contaminate later expressions or publish unresolved evidence.

Keep the full-module constraint pass authoritative. Retain complete constructor
evidence when compiling an already checked tool expression. Preserve checks
for unknown members, invalid generic arguments and nominal payload contracts.

Property providers cannot modify the type skeleton. Their return contracts
determine the property type. Register type-property evidence and its future
runtime binding from those contracts before module inference. Trait and
Property constraints can consume this static evidence without evaluating the
property value. Lexical evidence for generic parameters follows the same rule.

After static checking succeeds, evaluate property providers and materialize
their records. A failed provider, invalid capability or missing runtime record
prevents module publication; static evidence alone never authorizes publication.
Retain member-before-type evaluation, previous-property values and provenance.
Static type errors may now precede property-provider execution errors.

## Non-Goals

- Changing constructor, trait-selection or property applicability rules.
- Caching results solely by source location across different lexical scopes.
- Removing checks or treating incomplete metadata as successful evidence.
- Implementing RFC 0275 construction checks.
- Publishing or pushing this local experiment. Local main integration is required
  after completing and verifying the optimization.

## Verification

Use language cases for local and imported constructor providers, generic
shadowing, property-constrained traits, provider failures and diagnostics.
Run the workspace suite with a debug build. Compare identical declaration,
function and shared-type workloads against the preserved baseline executable.
Keep benchmark inputs and a reproducible runner with the change. Record actual
results and remaining architectural limitations before marking implemented.

The workspace debug test suite passes, including constructor tool evaluation,
generic lexical shadows and explicit type arguments, local/imported property
trait selection, static-error precedence and failed-provider publication.

Debug measurements on 2026-09-08, same toolchain and lockfile, one warmup and
three sequential samples per case (median wall seconds):

| Workload | 58f0b8c | Experiment | Speedup |
| --- | ---: | ---: | ---: |
| One Int constant | 1.203 | 0.802 | 1.50x |
| 400 Int functions | 4.550 | 2.201 | 2.07x |
| 400 independent structs | 11.231 | 2.863 | 3.92x |
| 400-element array | 1.296 | 0.874 | 1.48x |

Reproduce with `python3 scripts/measure-tool-inference.py BASELINE_BINARY
EXPERIMENT_BINARY --sizes 400 --samples 3`. The runner generates identical
temporary workspaces and performs no builds. These results measure the combined
optimization, not the isolated contribution of static property evidence.

## Remaining Limits

Tool expressions still have isolated inference state; this is shared input
preparation, not a single inference invocation for every phase. Partial and full
module analysis remain separate. Metadata-computing helpers still require tool
evaluation and local inference. Property planning retains the existing explicit
provider return-contract requirement. Shared type-DAG traversal is not globally
memoized. These changes do not establish a speedup for every workload.

## Follow-up: Scoped Type Environments

Strict inference and provisional type projection now borrow their parent
environment when entering closures, blocks and pattern branches. Local bindings
use a small Vec searched from the end; missing local type information explicitly
hides the outer binding instead of falling back to it. Rebinding changes only
the current scope, and dropping a scope leaves its parent unchanged.

The module environment remains a HashMap. Match joins that freshen inference
variables still construct a transformed environment: that operation changes
visible descriptors, unlike ordinary lexical scope entry. Large individual
local scopes may eventually need an index; the common small-scope path does
not allocate a hash table. Local scheme stacks are unchanged.

Unit tests cover borrowed descriptor identity, nested shadowing, removal and
reinsertion, and visiting each visible binding exactly once.

The workspace suite passes with 271 core tests and the language acceptance
fixtures. Release builds were compared with the existing benchmark runner,
`--sizes 400 1600 --samples 3`, after builds and tests finished. Median seconds:

| Workload | Before scoped environments | HashMap local scopes | Vec local scopes |
| --- | ---: | ---: | ---: |
| One Int constant | 0.146 | 0.134 | 0.133 |
| 400 Int functions | 0.399 | 0.226 | 0.230 |
| 1600 Int functions | 3.232 | 0.774 | 0.767 |
| 400 independent structs | 0.524 | 0.511 | 0.517 |
| 1600 independent structs | 3.868 | 3.850 | 3.870 |
| 1600-element array | 0.152 | 0.141 | 0.139 |

Eliminating full environment copies accounts for the substantial function
speedup. These samples do not show a clear additional speedup from Vec versus
HashMap local scopes. Independent type-declaration scaling remains unchanged;
dependency indexing and partial/full analysis reuse are outside this follow-up.

## Follow-up: Type Declaration Scaling

Tool evaluation now borrows module bindings and overlays generic parameters,
recursive family witnesses and provider arguments. Tool compilation returns
the actual external names needed by the lowered expression, including HIR
constructor references and hidden declared-owner links. Only those values enter
the VM external environment. Static inference still sees explicit type inputs.

HIR child indices replace whole-module scans for subtree dependencies, and
source-location lookup uses the existing sorted expression order. A single
iterative SCC decomposition identifies recursive type groups; the full module
schedules groups with dependency counts and a deterministic ready queue.
Partial analysis uses the same component classification and dependency order,
while retaining its existing recovery loop and failure propagation.

Verification includes all 512 three-node directed graphs, a 2048-node forward
chain, subtree-query equivalence including decorators, and the workspace suite
(274 core tests plus language acceptance fixtures). The benchmark runner adds
optional `--workloads forward-types repeated-family` to distinguish scheduling
costs from repeated applications of a memoized type family.

Release measurements against `b01c06e`, one warmup and three sequential samples
per workload, median wall seconds:

| Workload | Before | After |
| --- | ---: | ---: |
| 400 independent structs | 0.517 | 0.309 |
| 1600 independent structs | 3.804 | 0.957 |
| 1600 Int functions | 0.766 | 0.486 |
| 400 forward type aliases | 0.391 | 0.194 |
| 400 applications of Box(Int) | 0.440 | 0.252 |

The binding-only intermediate build measured 1.214 s for 1600 independent
structs, before the dependency/index changes. The main comparison used
`--sizes 400 1600 --samples 3`; the two additional workloads used `--sizes 400
--samples 3 --workloads forward-types repeated-family`. These improvements do
not imply that all inference work is now linear or globally shared.

### Duplicate Inference Audit

Interpreter memoization caches runtime results by function identity and
canonical argument TypeIds within the current heap. It does not cache tool
expression inference, bytecode preparation or environment construction.

Partial and full analysis still create separate ToolEvaluators. Within full
analysis, tool expressions receive constructor inference before execution, and
their ASTs can be checked again by authoritative module inference. Provisional
type projection also visits value bodies before strict inference. These are
remaining repeated computations, not eliminated by this follow-up.

The architectural target is one module constraint context, with tool queries
contributing evidence and recovery consuming the same analysis facts. It is
not sound to substitute a source-location-only cache: lexical witnesses,
expected types, substitutions and declaration completeness can differ between
visits. This change does not implement global constraint solving or claim to
remove those stage boundaries.

The solver itself already uses fresh inference variables, a substitution map,
bidirectional expected types and unification. Unknown callees receive a
function skeleton with fresh parameter and result variables. However, `resolve`
recursively follows substitutions and reconstructs many composite descriptors;
`occurs` invokes resolution again. A shared node-based solver could separate
shallow representative lookup from final descriptor materialization. Such a
change must preserve occurs checks, per-use generic instantiation, lexical
generalization boundaries, trait obligations and rejection of unresolved
inference variables at publication. It is not implemented by this optimization.

## Follow-up: Slot-Based Inference (In Progress)

The next solver representation uses a dense vector of small Copy nodes:

```rust
enum InferenceNode {
    Unknown,
    ProxyTo(InferenceVariableId),
    Known(InferenceTypeId),
    Conflicted,
}
```

`Known` identifies an inference-time type structure, not necessarily a completed
runtime TypeId. Its table entry describes a constructor and argument edges;
those edges may reference unknown or proxy slots. Thus Array(?element) has a
known structure before its element is solved. Equal slots share a representative
with iterative path compression. Solving an argument does not reconstruct its
parent structures. Array-scanning normalization passes can shorten edges before
publication; unresolved and conflicted slots must not become runtime TypeIds.
Generic parameters retain their lexical quantification scope.

Compatibility is not equality. In particular, Never may satisfy an expected
type without becoming equal to it. Structural projection and nominal adaptation
also must not create equality edges merely because a check succeeds. Conflict
details live outside the POD node; aliases observe the representative's first
conflict, and failed slots cannot subsequently be rebound as successful types.

The working implementation currently has dense proxy nodes, shallow head
inspection, an iterative occurs check, conflict tracking and deferred expression
normalization. Structural descriptor children are lowered to stable slot edges,
with a separate inference type table; their IDs are deliberately distinct from
runtime TypeIds. A final graph pass coalesces structural constructors whose
argument slots have become equal.
This includes partially known structures, not only fully concrete types.
Nominal completion and lexical parameters are excluded from this structural
coalescing pass. The descriptor-facing solver interfaces and repeated recursive
materialization still need replacement with graph operations. This intermediate
state is not the completed optimization.

The type table now stores 12-byte Copy rows consisting of a constructor ID,
argument start, and argument count. Argument edges are 4-byte slot IDs in a
separate dense vector. Constructor metadata is interned independently; nominal
arguments and bodies are edges too, not descriptor trees in the type row.
Shared authored declaration bodies are imported once by Arc identity. Lazy
descriptor views remain only as compatibility adapters for older consumers.

Known matching structural slots unify using an explicit integer work stack.
Occurs checks, conflict propagation, structural equality, and coalescing traverse
the flat edges directly. Coalescing uses an iterative postorder followed by a
single constructor/root-argument interning pass, avoiding one whole-array scan
per depth for forward-allocated structures. Tests cover 16384-level unification
and forward-edge normalization and assert that these operations do not create
descriptor views. Never compatibility remains separate from equality.

The descriptor adapter caches normalized nominal bodies by stable type-row ID
and solver revision. Every binding, alias, and conflict mutation advances that
revision; cached bodies cannot outlive new inference evidence. This avoids
repeated materialization during publication while descriptor-facing consumers
are being migrated. A regression test checks both Arc sharing within a revision
and invalidation after solving an unknown body argument.

### Real Query Bottleneck

The ontology `@test/query` workload initially took 48.52 s in release mode.
The proxy representation alone measured 47.40 s, so it did not explain the
dominant regression. Opt-in instrumentation subsequently measured 0.58 s in
descriptor normalization and 0.016 s in unification, compared with 35.3 s in
expression inference (these categories overlap and must not be summed).

HIR now prepares definition dependencies once after resolution, and reference
location lookup uses the already sorted reference array. Those changes remove
repeated searches but did not materially reduce this workload's runtime.

Exclusive expression measurements localized 34.09 s to `if` inference. Branch
freshening was replacing a few inference variables by recursively copying every
visible binding, including unrelated nominal type bodies. An intermediate version
borrowed the parent environment and overlaid only affected bindings.
That instrumented release workload fell from 48.15 s to 16.46 s; `if`
exclusive time fell to 3.18 s. These are sequential single samples, not medians.
Peak RSS did not improve (approximately 743 MB before, 769 MB after).

Branch freshening is now removed entirely: `if` and `match` use the same outer
slots, and scoped environments contain only actual local bindings. The old
environment traversal API, variable replacement walker, and replacement-evidence
merge are deleted. Complete expected types still provide branch context;
incomplete result contexts are checked after joining branch results, rather than
letting the first branch fix a common result slot. Structural join evidence and
compatibility remain distinct from slot equality. A sibling-scope regression
test checks shared binding identity and visibility of a late slot solution.
Temporary timing instrumentation has been removed from inference hot paths.

After removing branch freshening, the uninstrumented release query completed in
12.98 s and 13.39 s in two sequential runs (exit status 0), with peak RSS of
742844 KB and 769116 KB respectively. This is about 3.6-3.7 times faster than
the original 48.52 s sample, without a demonstrated memory improvement.
The final `cargo test --workspace` run passed, including 278 core tests and the
language acceptance fixtures; `cargo build --release` also succeeded.

The subsequent flat-table migration measured 15.91 s and 15.48 s in two
sequential release runs, with peak RSS 754276 KB and 754888 KB (exit status 0).
It is not an end-to-end speedup over the no-copy branch version. The final
workspace run passed 283 core tests and all language acceptance fixtures, and
the release build succeeded. Expression records and publication still use
descriptor-facing interfaces; those remaining graph/descriptor conversions must
be removed before treating the requested global-slot optimization as complete.

### Integration With Updated Main

The optimization branch now includes main's `915ffe4` (RFC 0275) through merge
`94555da`. Construction checks retain their runtime type witnesses and tool-stage
elaboration. Prepared construction evidence is reused instead of inferring the
same expression again, and tool bindings use borrowed scopes. HIR distinguishes
construction-check arguments from deferred property roots: a construction check
must be available before constructing its target. Static property evidence skips
`@check`, and slot-backed identity lookup preserves `Unchecked` idempotence.

The merged version passed `cargo test --workspace` (286 core tests plus language
acceptance) and `cargo build --release`. An independent release build of main
`915ffe4` ran the absolute-path ontology query in 197.05 s with peak RSS 607824 KB;
the merged optimization branch took 12.12 s with peak RSS 603724 KB, then 11.89 s
with peak RSS 603556 KB on a repeat run. All exited successfully. These are
sequential measurements, not medians, and must
not be mixed with the older-main measurements above. The final merge back into
main remains pending completion of the remaining solver-interface migration.

The requested relative workspace path currently fails workspace membership
validation; measurements use the equivalent absolute ontology workspace path.
Remaining work includes the descriptor-facing inference interfaces and fully
consuming resolved IDs through lowering rather than recovering them from AST
locations. The performance improvement does not imply a completed HIR-to-LIR
pipeline migration.

### Expression Record Experiment

An experiment imported descriptor results into slot-based expression records.
Workspace tests passed (288 core tests plus language acceptance), but the release
query regressed to 23.39 s with peak RSS 982356 KB (single sample, exit 0).
The record conversion was withdrawn: while inference still returns descriptors,
importing every expression result adds graph construction and storage overhead.
Direct slot-producing inference must precede this publication change.

Immutable zero-argument type rows are now interned per constructor; their mutable
slots remain independent. A regression test checks row reuse and slot isolation.
After withdrawing the record conversion, all 287 core tests and the release
build passed. The release query returned to 12.11 s with peak RSS 602556 KB
(single sample, exit 0). This does not establish a speedup from leaf interning
over the earlier 11.89-12.12 s samples. Workspace acceptance passed before the
withdrawal; the final reduced change was rechecked with the core suite.

### Direct Aggregate Inference

Array and tuple expressions now allocate constructor rows directly from child
slots and return their root slot. They no longer construct a full aggregate
descriptor first. Other expression families and final publication still retain
descriptor adapters; the expression-record conversion experiment above remains
withdrawn.

Known slots are shallowly exposed before nominal/name compatibility checks,
so an imported recursive name follows the same rules as an inline name without
requiring a local body lookup. Runtime `Never` evidence detection traverses slot
edges without constructing descriptor views. Freshening empty-container evidence
does not change the original `Never` slot. Tests cover direct expression edges,
late conflicts, known-name representation equivalence and `Never` isolation.

The workspace suite passed (291 core tests plus language acceptance) and the
release build succeeded. Two sequential release query samples took 12.51 s and
12.70 s, with peak RSS 653816 KB and 653644 KB (exit 0). Compared with the prior
12.11 s / 602556 KB sample, this is an intermediate migration cost, not a net
performance improvement. Remaining descriptor-producing expression families
must be migrated and measured before final integration.

Ordinary struct and Dict expressions subsequently migrated to direct constructor
rows as well, retaining canonical field ordering and child slot identity.
Workspace tests (291 core tests plus language acceptance) and the release build
passed. The release query took 12.78 s with peak RSS 658832 KB (single sample,
exit 0). This still does not recover the pre-migration memory baseline; further
work must address remaining mixed representations, including function
instantiation and publication, rather than claim a completed optimization.

### Slot-Producing Functions And Records

Function instantiation now substitutes parameter slots directly into constructor
rows, including shared nominal bodies within an instantiation. Closure results
also use Function rows. Expression inference returns its recorded slot rather
than returning a descriptor that its parent must import again. Expression records
therefore contain slot IDs, while contextual conversions replace only the
expression edge. Recursive result equations and constructor payload refinement
shallowly expose known slots before inspecting their shape.

The remaining per-node storage was compacted: `Conflicted(u32)` references a
shared message table, reverse dependencies use flat integer links and O(1) list
splicing on proxy merges, and normalized body caches allocate entries only on
use. New nodes do not invalidate existing normalized bodies; binding, alias and
conflict changes still advance the solver revision.

An intermediate slot-record version took 22.29 s and 1247608 KB on the release
query. Temporary arena statistics found one inference state with 1499197 slots,
1203188 type rows and 138923 body imports. Concrete nominal bodies now reuse an
import by resolved declaration identity and exact body-view equality. Unknown
and bound parameters are excluded; different recursive views remain separate.
This targets repeated imports caused by normalization returning different Arc
addresses for the same body. Constructor pattern evidence is retained when a
recursive nominal stub alone cannot reveal the payload shape to pattern analysis.
Temporary instrumentation has been removed.

Final publication still uses descriptor adapters, and inference still has
name-facing environment interfaces. These remain migration work before main
integration; compact storage alone does not complete those interfaces.

The final workspace suite passed (298 core tests, 41 CLI tests including language
acceptance), and the release build succeeded. Uninstrumented sequential query
samples took 11.04 s and 11.17 s, with peak RSS 754916 KB and 755484 KB (exit 0).
This reduces wall time from the previous 12.78 s sample, but does not recover its
658832 KB memory baseline. These are samples, not medians. Rigid bound parameters
retain nominal diagnostics, and nominal argument compatibility does not directly
equate the actual value slot with a refinable instance parameter.

### Direct Structural Publication

Final structural expression types now publish iteratively from shared slots into
the TypeGraph, with an array cache of final IDs and publication failures. Shared
children are published once; ordinary expression descriptors are no longer
materialized and retained for the entire module. Nominal descriptors needed by
runtime owner bridges, exported scheme overrides, and the final result remain.
Binding nominal identities are reserved before expression publication.

Declared types and pending alternatives still normalize through the compatibility
adapter. This preserves parent-before-body recursive nominal reservations,
Unchecked normalization and alternative collapse. It is not yet direct nominal
publication or a completed HIR-ID environment migration. Tests cover a 16384-deep
structural graph without descriptor views, shared roots, unresolved/conflicted
slots, combined publication failures, Bound parameters and nominal normalization
equivalence.

The release build succeeded. Two sequential query samples took 9.89 s and
9.74 s, with peak RSS 393428 KB and 394940 KB (exit 0). Compared with the prior
11.04-11.17 s / 754916-755484 KB samples, structural publication removes nearly
half the peak resident memory and also reduces wall time. These remain samples,
not medians. The workspace suite passed with 301 core tests and 41 CLI tests,
including language acceptance.
