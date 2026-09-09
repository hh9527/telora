# RFC 0280: Session-Wide Type World and Execution-Free Inference

- Status: Revised accepted direction; implementation incomplete.
- Revision: 2026-09-09, replacing the module/tool publication model with the
  session boundary clarified by the user.
- Tracking: [#175](https://github.com/hh9527/telora/issues/175).
- Branch: `feat/0280-arena-type-consumers`.
- Related: RFC 0276, RFC 0277, RFC 0279; performance investigation in #173.
- Evidence: [historical measurements](0280/measurements.md), including the
  regression at checkpoint `293098c`. No performance claim for this redesign.

## Decision and Scope

Create module definitions, imports/exports, type terms and inference slots in one
session-owned world from the beginning. Consumers refer to these records by
typed integer IDs while they are being resolved. A module is a naming and
dependency boundary, not a transaction, type-publication or ownership boundary.

The primary abstraction is one information graph progressively solved and
refined, not a collection of stage caches. Allocate identity before solving
content; resolve adds reference edges, inference adds constraints and solutions,
and downstream consumers follow the same identities. A pending dependency queues
work on that graph rather than requesting another module analysis. Complete all
statically decidable facts before user-code execution (including entry); dynamic
values remain explicit typed execution obligations. Source/name lookup indexes
are ingress aids, not alternative owners of inferred facts.

The current implementation focus is workspace/package mode. Standalone mode is
temporarily out of scope; do not add standalone-specific adaptations or acceptance
fixtures as part of this work.

The restriction against partial publication applies to final user-program
results and execution capabilities leaving the session. It does not prohibit
registering, sharing or querying incomplete type/module definitions internally.
Unknown, proxy, pending and conflicted facts are legitimate internal states.
An error does not require undoing a module's registrations or copying its
environment to protect other modules.

The session output boundary does not make the internal world a session-wide
transaction either. Retain failed and incomplete analysis records for diagnostics;
withhold the final successful user result, rather than rolling those records back.

Static type elaboration and inference must execute no Telora code, including
builtin Telora source. Execute user values, property configuration/providers
and other runtime obligations only in later value phases. Keep their dependencies
and source locations during analysis; do not evaluate them to discover type shape.

This explicitly expands the earlier RFC's scope to session ownership, reusable
source/HIR artifacts, static module interfaces and delayed value initialization.
The former exclusion of module preparation and global inference organization
is superseded. Graph-backed schemes, nominal argument keys, trait/property,
pattern and tool consumers remain required; they now share the session world
instead of repeatedly importing and publishing separate type graphs.

Preserve existing type rules and public Rust descriptor contracts through
boundary adapters. Changes to observable initialization/diagnostic ordering must
be identified and tested as part of delaying execution, not hidden behind a claim
that the old scheduler is unchanged. No new surface syntax is proposed.

## Why the Direction Changes

The previous approach reduced individual normalizations and then published an
owned graph for each tool expression. It still retained the sequence
"infer -> copy/publish graph -> materialize descriptor -> construct metadata".
Its smaller allocation count inside one function did not establish an overall win.

At checkpoint `293098c`, property-400 normalization allocation stacks decreased
by 25.04%, but total allocations increased by 0.76% against baseline and peak
heap increased from 37.89 to 38.20 MB. Reversed-order ten-sample comparisons
showed approximately 1.6–4.0% slower medians on the large synthetic cases.
The checkpoint passed correctness tests but fails the performance gate.

The target is therefore to remove unnecessary work and ownership transitions:
register definitions once, infer on shared slots, retain results for all consumers,
and never execute Telora merely to turn static type syntax into metadata and
decode it back into a type. The measurements motivate this design; they do not
prove a speedup or justify predicting one.

## Current Implementation Evidence

Paths below are relative to `crates/telora-core/src/`, inspected at `293098c`.

| Area | Existing behavior | Required direction |
| --- | --- | --- |
| `module/graph.rs`, ModuleGraph::discover | Discovers reachable imports, export plans and declaration slots without VM execution; temporary parse artifacts are discarded | Retain sources and parsed/HIR artifacts in the session |
| `module/graph.rs`, MainWorld::with_modules | Already reserves function and concrete nominal identities before module evaluation | Extend the shared owner to symbolic types, interfaces, inference and evidence |
| `module/loader.rs`, compile_telora | Reads/parses a discovered module again | Consume the retained artifact for that source snapshot |
| `module/loader.rs`, load_resolved_value | Obtains an imported interface together with an executed module's values | Resolve import/export definition IDs without initializing values |
| `types/dependency.rs` | Executes type bodies, families and annotations, then decodes metadata into types | Elaborate restricted type syntax directly into session type terms |
| `types/traits.rs`, evaluate_type_constraints | Evaluates the type operand of Property(P) | Resolve P as a static type reference |
| `types/type-boundary.rs` | Rejects ordinary helper results, metadata variables and .type values in type positions | Reuse this existing language boundary for execution-free elaboration |
| `types/properties.rs` | Presence follows provider return contracts; target applicability can still be computed | Separate static presence/type checking from deferred value validation |
| `types/descriptor.rs`, `value.rs` | Schemes and nominal argument identities contain recursive descriptor/TypeExprId trees | Use shared templates and integer argument references internally |

The existing inference arena already has POD state nodes, constructor rows and
flat argument edges. TypeGraph and TypeStore also contain integer child edges.
Reuse those mechanisms; changing the lifetime/owner is essential, renaming
TypeExprId without removing its recursive boxes is insufficient.

## Session Model

A session is one preparation/analysis/execution context for selected roots and
their resolved sources. It can be owned by the building MainWorld or another
global resource with the same lifetime. It is not necessarily the lifetime of a
process or an LSP connection. The concrete Rust type name is an implementation
choice; the ownership and visibility rules are not.

```text
SessionWorld
  sources / parsed modules / HIR
  module catalog / reachable module graph / definitions / imports / exports
  symbolic type terms / inference slots / templates
  constraints / diagnostics / trait and property evidence / value obligations
  compiled functions / runtime type store / initialized values

  internal registration and resolution are immediately shared by ID
  final user-result publication is a separate session operation
```

The session can expose an incomplete definition to another module. Consumers
must inspect its state, enqueue dependencies when needed, and propagate conflicts
as facts. Do not require a "validated module interface" or immutable copied graph
before making its definition IDs visible. Registration does not promise that the
definition, module or eventual program is valid.

A module failure leaves its IDs and diagnostics available so independent work
can continue. Finalization consults explicit failures and unresolved obligations;
absence of a value or absence of a Known node is not proof of success. No
per-module rollback, commit protocol, or separate arena snapshot is required.

Analysis/query results may report incomplete or conflicted facts for tooling.
Such reports are not publication of a successful user-program result. Preserve
the command's diagnostic/recovery protocol; do not hide diagnostics until success.

Previously published sessions remain independently owned. Reuse across sessions
must carry owner/revision identity or import/remap once at that actual boundary.
It must not turn every module within a session into a separate transaction.

## IDs and Type Storage

Use typed indices for modules, declarations, expressions, inference slots and
type terms. Imports, aliases and re-exports point to the same declaration records;
namespace qualification changes lookup/display information, not the underlying
type tree. A module may initially have only a name and empty/pending table ranges.

An export first identifies a declaration, not an initialized runtime value.
Keep its definition/type reference separate from its value-initialization state.
Resolving an import can therefore return that declaration ID while its type is
still open and its value has never executed. Later initialization fills the value
record without rebuilding the import/export tables or changing declaration IDs.

IDs remain stable while a session is filled and resolved. Finalization must not
renumber all definitions or rebuild module tables. A binder-aware generic
instantiation allocates fresh slots, while a type alias or import reuses identity.

```text
slot[id] = Unknown | ProxyTo(slot_id) | Known(term_id) | Conflicted(diagnostic_id)
term[id] = (constructor, argument_slot_ids / payload_range)
template = (binder_id, parameter_ids, constraints, body_reference)
```

Known(term_id) means a constructor is known; its children can still be unresolved.
It is not necessarily a final runtime TypeId. Equality constraints link slots to
a representative, with path compression. Directional checking, Never and nominal
context refinement keep their current semantics; they are not arbitrary equality.

A call can solve T structurally and a later callback can refine its nominal
evidence. Preserve the shared slot through both call and closure checking.
Conversely, equal current types do not permit merging independent generic
instantiations or binders. Lexical shadowing does not copy the global type world.

Resolve and canonicalize edges in array passes and affected-node work queues.
Do not normalize the entire session after every constraint. Reuse unchanged
facts; re-query or invalidate facts after relevant proxy/binding/conflict changes.
Head queries and graph predicates must not materialize descendant trees.

Nominal identity uses declaration/constructor identity plus canonical argument
references. Symbolic arguments can contain Bound or unresolved slots. Do not
freeze a hash key from mutable slot contents; resolve/rekey when its dependencies
change. Equivalent applications share canonical terms after resolution.
Reserve recursive identities before processing their bodies, while allowing
internal references to those still-open bodies.

Remove internal recursive TypeDescriptor/TypeExprId argument duplication.
Runtime TypeStore continues to own concrete TypeIds, including its Unchecked
encoding. Symbolic terms, inference slots and runtime IDs can live under the same
session owner without being interchangeable numeric domains.

Initially require stability within a session. The current sorted-name ModuleIds
are deterministic for a fixed graph, not stable across arbitrary graph edits.
Cross-revision persistent numbering is a later registry policy; this RFC must
not claim it merely because records use u32.

## Pipeline

### 1. Catalog and reachable module discovery

Prepare a name/catalog index for resolution without parsing or evaluating every
available module. Starting from entry/query/test roots, discover the reachable
static imports, implicit prelude and applicable builtin/Host modules.

Retain one source snapshot and parse/HIR artifact per reachable module. Register
module/declaration IDs and import/export relationships as discovery proceeds,
then resolve pending references once enough of the graph is known. Preserve
existing resolver visibility, aliases, explicit exports and native restrictions.

### 2. Direct static type elaboration

Translate type syntax into session terms: primitives, Unit/tuple, function types,
Array/Dict, declared struct/enum/newtype, aliases, recursive types, type families
and their bounds. Symbolic family substitution uses the shared template and
binder-aware slots; it does not execute a family body for each concrete argument.

Builtin types and Host contracts enter as trusted static definitions/signatures.
Builtin Telora bodies can be parsed and checked, but must not run to establish
these interfaces. Static data inputs may use their native format decoders; this
is not permission to initialize imported Telora modules.

The static-analysis API has no prerequisite of initialized exports, ToolEvaluator,
VM or heap metadata. There is no execution fallback for type syntax. Migration
may retain an explicitly incomplete legacy path temporarily, but cannot report
the new boundary as delivered while it can enter the VM.

### 3. Session-wide inference and resolution

Infer expressions and function bodies, including unannotated exports, against
the shared declarations/templates. "Type phase" includes statically checking
value expressions; it does not mean only reading type declarations.

Module/SCC queues organize work, but constraints and results reference the same
session records. A queue waiting on another definition does not trigger module
execution, environment cloning or graph publication. Resume affected constraints
when new facts arrive. Preserve current rejection of unsupported module value
initialization cycles; shared type registration does not legalize them.

Record expression types, arities, constructor ownership, interpolation evidence,
trait dictionaries and runtime-type requirements by ID. Strict analysis and
error recovery consume these facts; do not independently repeat a whole inference
pass merely to translate between their representations.

### 4. Deferred value and metadata work

Type solving records obligations, not computed user values. For example:

| Construct | Static phase | Later value phase |
| --- | --- | --- |
| T.type | Resolve T and infer TypeOf(T) | Materialize the required metadata witness |
| @provider / HasProperty | Infer provider contract and establish the property type/presence relation | Execute configuration/provider and construct property data |
| @property(target(True)) | Check target's expression type is PropertyTarget; record applicability obligation | Execute target and validate its actual capability |
| @check | Check Result((), BlameError) contract and record checker identity | Initialize the checker and run it at the existing construction/codec boundaries |
| Trait implementation | Check member contracts and select static evidence | Initialize needed implementation values |

The dynamic property-target example is supported by the existing
`tests/language/src/test/property-target/testee.telora` test. Do not restrict it
to literals to make static inference appear execution-free. Its deferred
applicability failure invalidates successful user output; it does not require
removing its type or module from the internal world.

After static inference, compile the required value work to LIR/bytecode and
execute according to its dependency plan. Sharing interfaces no longer requires
initializing every dependency first. Preserve required initialization, provider
ordering, check behavior, quotas and source provenance. Inventory and test any
observable ordering change caused by moving execution out of analysis.

Metadata construction consumes solved graph roots directly, sharing conversion
within the relevant session/heap/binder context. Metadata carries property values,
lexical witnesses and source provenance; a bare TypeId does not replace it.
Descriptor materialization remains only at explicit compatibility boundaries.

### 5. Entry execution and final user output

The selected entry drives user-program execution. Before exposing a successful
result or executable capability, verify the relevant static failures, unresolved
slots, pending value validations and initialization failures are discharged.

This is a session output gate, not a requirement to seal each module's definitions
before use. Diagnostics and typed recovery facts can still be emitted as such.
Do not claim successful `check` from pure inference alone: the existing command
also performs initialization and validation. Existing test/error reporting and
Host effect protocols remain distinct from internal definition registration.

This gate does not promise to undo externally visible Host effects that have
already occurred during value execution. Preserve the existing effect protocol;
any stronger buffering or transactional guarantee needs a separate design. It
must not be implemented by isolating or copying internal module/type records.

## Reachability and Tree Shaking

Imports/exports help build symbol reachability but are insufficient by themselves.
Include entry/test roots, implicit dependencies, closures, trait/property evidence,
metadata use and required initializer effects. Share definition/type records even
when some runtime code will not be emitted.

The immediate optimization is delaying execution and eliminating reconstruction,
not changing which user effects occur. Do not silently drop warnings, failures,
checks or static errors in unused code. Effect-aware removal of value work needs
its own demonstrated correctness; aggressive tree shaking is not a prerequisite
for delivering the shared session type world.

## Consumer Migration and Compatibility

Schemes and internal module interfaces become views of session tables. Trait,
property, pattern, tool and compiler consumers use those same references.
Dropping a per-module solver queue must not destroy their types or force them
into an owned descriptor tree. Keep source locations and non-type evidence in
separate tables with the same session lifetime.

The existing public TypeScheme, ModuleInterface, DeclaredTypeId and TypeNode
contracts may retain explicit ingress/egress adapters. They must not remain the
authoritative trees copied between internal modules. Cache external imports once
per valid source owner/session, not once per expression or importing module.

The graph publication code and tool-owned graph from the prior checkpoint can
serve as temporary compatibility mechanisms. They are not the final architecture
or mandatory module acceptance gates. Replace or remove them where a shared
session reference suffices. Retain useful allocation-free queries and the
call/closure correctness fix.

Pack hot variable-sized fields/arguments into flat payload tables when measured
cost justifies it. Strings, source text and runtime metadata need not become POD.
Logical sharing and removing execution dependencies take priority over packing.

## Implementation Plan

Each milestone needs a reviewable diff, its specific tests and updated evidence.
Commit this revised RFC before implementing the changed phase architecture.

1. Preserve the baseline and `293098c` measurements and extend
   [the benchmark runner](../scripts/measure-tool-inference.py) with module-graph
   workloads. Add opt-in counters for parse
   and HIR construction, definition registration, interface preparation, graph
   imports/materializations, constraints and VM entry by phase. Default builds
   contain no instrumentation. Keep the prior regression visible.
2. Introduce the session owner and module/declaration/import/export tables.
   Reuse discovered source/parse/HIR artifacts; share IDs before completion.
   Prove cross-module access to pending/conflicted records without rollback.
3. Implement direct static type elaboration and static builtin/Host interfaces.
   Separate type/interface lookup from imported value initialization. Cover all
   supported type syntax, recursive/family templates and property constraints.
4. Move inference slots, templates, nominal argument keys and consumers into the
   shared world. Use module/SCC work queues over shared facts; remove per-module
   environment/interface copies and duplicate strict/recovery inference.
5. Move remaining Telora execution to deferred value phases. Preserve obligation
   identities, provider/check/trait behavior and runtime provenance. Build metadata
   directly from required graph roots and enforce the session user-output gate.
6. Remove obsolete internal tree round trips and temporary tool/module graph
   publication paths. Audit external adapters, retained memory and invalidation.
   Profile before deciding on further payload packing or tree shaking.
7. Run full semantic and performance acceptance. Merge the completed branch into
   main and close #175 only after the gates pass. Correctness-only intermediate
   checkpoints, including `293098c`, do not satisfy delivery.

## Acceptance

### Architecture and ownership

- A discovery fixture with diamond imports, aliases and re-exports parses/builds
  HIR once per module source snapshot and shares declaration IDs. Unreachable
  catalog entries are not parsed. Imports resolve before dependency values run.
- Another module can refer to an Unknown/open type, observe its later solution
  or conflict, and continue independent work after an error. Assert no type-table
  rollback, full environment copy or per-module graph publication in this path.
- Internal incomplete definitions remain queryable; no successful user result
  or executable capability escapes a failed session. Recovery diagnostics retain
  their protocol. Cancellation/stale-session tests leave prior published results
  intact without introducing per-module transactions.
- Test zero VM entries throughout the pure static API, including imported and
  builtin Telora sources, annotations, families, trait/property constraints,
  expression-body inference and statically failing programs. Its dependencies
  must not execute Telora indirectly.
- Instrument a deferred property-target/provider helper: zero invocations during
  inference, expected invocations and values later, and a failing control that
  blocks successful output without erasing registered definitions.
- Tool/analysis/compiler facts survive release of solver scratch state through
  session ownership. Separate sessions with overlapping numeric IDs remain
  isolated. Finalization does not renumber/rebuild internal definition tables.

### Semantics and representation

- Differential head/predicate tests cover primitives, wide/shared/deep graphs,
  aliases, nominal bodies/arguments, Bound, Never, Unchecked and alternatives.
  Preserve full versus exposed unresolved-variable traversal.
- Late binding, proxy merges, known-slot refinement and propagated conflicts
  update all dependent consumers. Query caches invalidate appropriately. Deep
  traversals are iterative and do not reconstruct descendant descriptor views.
- Distinct generic calls/binders remain independent; equal types do not imply
  equal variables. Cover nominal callback refinement, lexical shadowing and
  cross-module templates. Equivalent applied nominal arguments canonicalize
  together after resolution without recursive identity-tree duplication.
- Recursive identities are reserved before body traversal. Open stubs do not
  hide incomplete bodies or become valid concrete runtime metadata. Test cycles,
  remapping at actual external boundaries and shared subgraph reuse.
- Verify actual property/check/trait/interpolation behavior, .type diagnostics,
  constructor owners, checked construction/codec failures and provenance.
  Internal scheme/pattern/tool paths must consume graph references; inventory
  every remaining descriptor adapter and its cost/lifetime.
- Run `cargo test --workspace`, release build, `git diff --check` and source-size
  checks. Follow [TESTING.md](../guide/TESTING.md): successful check is not a
  behavioral assertion; warnings and ordinary returns are not failure assertions.
  Report existing formatting differences without unrelated churn.

### Performance

Use identical inputs and equivalent successful outcomes, uninstrumented release
binaries, one warmup and at least five samples. Reverse version order or interleave
runs to examine drift; report distributions and rerun uncertain regressions.
Do not benchmark alongside builds, tests or profilers.

Include constant/small controls, plain/property at 100/200/400 and larger scales
when practical, shared wide/deep types, generic calls, codec-schema, diamond
imports and many small modules. Report both pure static phase and end-to-end
prepare/check/entry costs; moving work later is not eliminating it.

Report CPU samples, allocation calls, cumulative allocated bytes where available,
peak heap, uninstrumented peak RSS, and retained memory across repeated sessions.
Also report nodes/edges, payload bytes, scratch/conversion-map peaks, parse and
interface counts, normalization/materialization and VM entries by phase. Inclusive
CPU/allocation stacks overlap and must not be added as separate phase budgets.
Cache improvements require hit/miss/invalidation evidence.

Acceptance requires reduced duplicate construction/materialization on targeted
shared-type/module workloads without repeatable material wall-time or peak-memory
regression on controls. No percentage gain is promised. If results remain within
noise or regress as at `293098c`, report that and continue investigating rather
than declaring the RFC complete.

## Alternatives Not Selected

- Bigger descriptor caches or a graph per tool expression: retain the ownership
  transitions and round trips exposed by the measured checkpoint.
- Per-module atomic type publication: adds copying/isolation that the session
  boundary does not require.
- Executing type syntax through a faster VM: preserves a dependency that the
  current restricted static type language does not need.
- One untyped global integer for all domains: loses the distinction between
  symbolic terms, mutable inference slots and concrete runtime identities.
- Copying branch environments, unconditional full-arena scans per constraint,
  or forcing all diagnostics/runtime values into the type arena.
- New module initialization-cycle semantics, unrestricted type-level functions,
  public union/Any, or changing public Rust fields without adapters.

## Current State and Evidence

The existing branch contains `ac91ae6` (arena queries/call edges) and `293098c`
(owned tool evidence, closure edge preservation and measured interim costs).
The latter passed 328 core tests, 41 CLI tests, 400 language groups and release
build, but regressed in performance. These results verify that checkpoint only,
not the architecture specified here.

The revised session pipeline, execution-free static API and global consumer
migration remain to be implemented. The earlier graph-to-metadata prototype
was not integrated. [Historical measurements](0280/measurements.md) preserve
the baselines, counter observations, raw artifact paths and regression evidence.
Current design documentation will be updated as implementation lands; this RFC
does not describe those new boundaries as already implemented.

### Session source preparation checkpoint

Module discovery now registers sources in the same SourceDatabase subsequently
used by loading and recovery. The session module graph retains shared
PreparedModule records (source ID, lowered program, recovery syntax and
diagnostics); strict and recovery loaders reuse them without reading/parsing a
discovered module again. Invalid overlay parses remain registered. Unneeded CST
storage is discarded. New tests verify snapshot reuse after disk edits and
retention of failed overlay syntax; both passed.

This is only the source/parse portion of milestone 2. HIR construction, global
declaration/type slots and execution-free inference remain outstanding. Existing
public semantic inputs still own AST copies; retained memory and end-to-end
performance must be measured before claiming a net benefit. Full workspace tests
and the release build passed. The [measurements](0280/measurements.md) show
approximately 4–5% lower medians for 400-type workloads against `293098c`,
smaller module-graph improvements and mixed small controls. Property-400 peak
heap decreased by 2.62%; broader retained-memory acceptance remains outstanding.

### Demand-driven recovery checkpoint

Workspace loading now attempts strict analysis first and uses its facts even
when subsequent compilation or value execution fails. Partial analysis is invoked
only when no strict Analysis exists, avoiding the former eager partial pass whose
result was discarded on success. This removes duplicate success-path work toward
milestone 4; it does not yet share partially solved strict facts with recovery or
remove Telora execution from either analysis API. Full workspace tests and release
build passed. Against source reuse alone, ten-sample medians improved by 20.86%
for typed-400, 16.87% for property-400 and 11.63–13.58% for 400-module graph
cases. Property-400 allocation calls decreased 16.69% and peak heap 5.48%.
Shared-wide-400 initially showed a 2.09% regression, which did not repeat in a
separate fifteen-sample follow-up; timing drift limits claims for this control.
This checkpoint does not establish the RFC's full acceptance. See the measurement
appendix for scope, artifacts and remaining gates.

### Import reference graph checkpoint

Discovery registers explicit import nodes before resolving their target names.
ImportId stays fixed while the flat node array is filled from Pending to a
ModuleId or a diagnostic ID. Module targets live in an ID-indexed table; strict
and recovery loaders consume these facts rather than resolving discovered paths
again. The source-location map is only an ingress index. Multiple aliases share
their target module; a conflict does not remove independent nodes.

Workspace/package tests cover aliases referencing an uninitialized dependency,
retention of a failed resolution despite a later catalog change, and independent
node completion in the presence of a conflict. This is a name-reference graph,
not yet the shared declaration/export/type graph. Module IDs still follow the
existing fixed-discovery numbering, and legacy non-discovery entry paths retain
their existing resolver fallback. No standalone-specific enhancement is included.

Full workspace tests (333 core, 41 CLI including language acceptance), release
build and diff/source-size checks passed. Two-order ten-sample measurements show
small 0.28–1.60% module-workload median reductions versus `b0aa34d`, not a major
speedup claim. Diamond-400 allocations decreased 0.69%, while peak heap increased
0.11 MB and RSS median increased 0.91%. Retaining the new graph alongside legacy
structures has a cost; the later declaration/type migration must eliminate that
duplication. Detailed evidence is in the measurement appendix.

### Shared artifact consumers checkpoint

The module skeleton now has a source identity edge. Strict loading of that same
source uses ModuleId directly, without cloning the skeleton or rebuilding its
declaration/export/import plans for equality checking. Existing non-discovery
entry validation remains. The source-change regression fixture now exercises a
workspace/package source snapshot.

Within strict and partial analysis, the tool inference context and enclosing
analysis share one HIR allocation. The current public Analysis receives ownership
after the temporary context is released; no HIR tree clone is used at that
handoff. This removes an ownership transition but does not yet resolve HIR before
imported value initialization or unify strict-failure recovery with the same
inference records. Full workspace tests (333 core, 41 CLI including language
acceptance), release build and diff/source-size checks passed. Ten-sample
two-order check medians changed by -0.17% to -1.10%, insufficient for a strong
speedup claim. Property-400 peak heap decreased 35.16 -> 33.83 MB (-3.78%)
and uninstrumented RSS median 48,036 -> 46,376 KiB (-3.46%). These check
workloads exercise HIR sharing, not the strict loader's skeleton shortcut.
The direct type-contract elaboration path remains outstanding.

### Direct declaration contracts checkpoint

StaticContractScope elaborates known type references, symbolic parameters,
functions, tuples/Unit and builtin Array/Dict/TypeOf directly into the TypeGraph
that later becomes Analysis.types. Its inputs contain no evaluator, heap or
runtime values. Builtin-name interpretation respects lexical/import/parameter
shadowing; unimplemented forms retain an explicit legacy contract path. A static
contract with unbounded generic parameters no longer creates parameter metadata.
The existing TypeScheme consumer still receives one descriptor at its adapter
boundary; moving that consumer to graph roots remains required.

Earlier graph roots can expose structural sharing through nominal recursion.
Descriptor traversal now tracks nominal boundaries instead of rejecting every
revisited structural ancestor. A pure structural cycle still fails. When a full
nominal descriptor arrives after a recursive Never-body stub, its body refines
the same row, preserving references already present in contracts. Tests cover
generic parameter sharing, shadowed families, nominal recursion, invalid
structural cycles and later nominal refinement. The initial serve regressions
were reproduced and fixed at this boundary, not hidden behind a VM fallback.

Type definition bodies, family application, bounds, imported interfaces and
property/value preparation still include legacy execution. This checkpoint is
not execution-free session-wide inference. Final workspace tests passed (338
core, 41 CLI including language acceptance), as did release build and diff/source
size checks. Against `b5464c4`, two-order ten-sample end-to-end check medians
improved 39.42% for shared-wide-400, 35.92% for shared-deep-400, 17.61% for
typed-400 and 12.83% for property-400; diamond-400 improved only 1.48%.
Property/wide allocation calls decreased 9.50%/36.44%, with peak heap reductions
of 4.05%/4.52%. These are incremental synthetic-workload results, not a claim
about ontology or completion of the RFC. See the measurement appendix.

### Qualified declaration contracts

The direct path also reads concrete type exports through nested module
interfaces (`pkg.inner.Item`). It requires a namespace, a declared type export
and an unquantified type witness; ordinary metadata-valued exports and type
families do not qualify. Lexical and generic parameter shadowing still takes
precedence over an imported namespace. No runtime value or evaluator is an input
to this lookup. Interface construction itself still depends on the existing
module pipeline, so this does not yet move resolution before imported value
initialization. The preceding timing table measures the earlier direct-contract
checkpoint, not this extension.

Workspace validation for this extension passed 339 core tests and 41 CLI tests,
including language acceptance. The static API test covers an authored namespace
import, nested interfaces, parameter shadowing and rejection of a metadata-valued
export lacking a type declaration. Test log:
`/tmp/rfc0280-qualified-contract-workspace.log`.

### Symbolic family application in declaration contracts

Eligible local and imported family schemes now register template roots in the
same analysis graph. Contract applications substitute argument IDs through those
roots without invoking a runtime closure or copying metadata. Nominal application
identity is reserved before following recursive body edges; phantom arguments
remain part of identity. Equal applications reuse interned roots, and an existing
nominal stub can acquire its complete body without changing ID. Substitution uses
a sparse per-application node map rather than cloning the type environment.

This is an application consumer migration. Template production still runs the
legacy type-definition pipeline, and nominal identity arguments still cross a
descriptor adapter. Constrained templates and unresolved named references retain
the checked legacy path. The full session graph must ultimately own the templates
before module value initialization; the module-local registration here is not the
final ownership model. The measurement runner includes local and qualified
family-contract cases to distinguish this change from type-body evaluation.

Full workspace validation passed (343 core, 41 CLI including language acceptance)
and release build passed. Against the qualified-contract checkpoint, two-order
ten-sample check medians decreased 34.20% for local family-contracts-400 and 36.31%
for qualified-family-contracts-400. Other controls moved between -2.79% and +0.87%,
without evidence of a general module-graph speedup. Qualified family allocation
calls decreased 26.44% and peak heap decreased 18.40%; see the measurement
appendix for scope and artifacts.

### Static declaration bodies and source provenance

The analysis graph and imported family roots are now established before the
declaration dependency schedule. Supported noncyclic declaration bodies, including
lowered struct/enum/newtype constructors, elaborate directly into that graph.
New family templates become available to later type definitions immediately.
Static unbounded family bodies no longer create parameter metadata merely to
execute a constructor. Concrete declarations retain their nominal identity and
canonical TypeStore registration.

Legacy metadata consumers still require an explicit materialization adapter.
That adapter now projects source-use origins from AST references and existing
family metadata onto generated values; locations are not attached to canonical
type identity. This preserves separate origins for structurally identical fields,
aliases, template members and substituted argument interiors. Metadata building
without origin projection does not construct origin paths.

Known source type references reuse their metadata objects at this adapter, with
the occurrence location carried by the referencing value. They do not overwrite
the shared object's origin. Symbolic parameters explicitly bypass this reuse so
a same-named outer type cannot replace a binder. The intermediate implementation
rebuilt these objects and increased property-400 peak heap despite reducing
allocation calls; the reported final measurements include the reference reuse change.

The first validation run exposed a lost codec rule location and three tests that
assumed static type syntax consumes VM fuel. The location regression is covered
at the codec boundary and by focused origin tests. Execution fuel tests now run
actual tool expressions on one shared account, while module quota coverage uses
a property provider. A separate test requires supported static type definitions
to succeed with zero execution fuel. This does not establish zero execution for
the entire pipeline: recursive definition components, unresolved/bounded forms,
construction/property preparation and failure recovery still have legacy paths.

The final workspace suite passed (349 core, 41 CLI including language acceptance)
and release build passed. Compared with `4eb1b22`, two-order ten-sample check
medians decreased 36.16% for types-400, 34.26% for repeated-family-400, 32.42% for
typed-types-400 and 26.33% for property-types-400. Constant startup decreased
12.70%, while module-diamond-400 decreased only 2.77%. Property allocation calls
decreased 22.80% and peak heap decreased 2.70%. See the measurement appendix for
the intermediate heap regression, final artifacts and scope limitations.

### Static constraint facts

Declaration and family signatures now attempt to resolve their trait identities
and `Property(T)` type arguments directly in the existing analysis graph. The
static constraint API has no evaluator, quota account, runtime values or property
providers. When both the body/contract and its constraints resolve statically,
quantified parameters do not need temporary runtime metadata. Duplicate constraint
diagnostics and canonical constraint ordering are retained.

Unsupported bounds retain the existing checked migration path. Recording a bound
does not prove that an application satisfies it: instantiation and property/trait
evidence checks remain required. In particular, constrained family applications
still use their existing checked path; the current family graph consumer does not
silently erase their obligations. Focused tests require trait plus property bounds
and constrained family signatures to succeed with zero execution fuel, and retain
the duplicate-property diagnostic. Local and qualified property-constraint
workloads were added to the measurement runner.

Full workspace validation passed (351 core, 41 CLI including language acceptance)
and release build passed. Against `7bd53b8`, two-order ten-sample check medians
decreased 21.62% for local property-constraints-400 and 23.89% for its qualified
variant. Controls moved between -1.67% and +0.33%, without evidence of a general
pipeline improvement. Qualified constraint allocation calls decreased 9.72%; peak
heap was approximately 12.06 MB in both versions. The measurement appendix records
workload scope and artifacts.

### Deferred construction dependency preparation

Static type bodies and declaration contracts no longer prepare the whole module's
construction/check value dependencies before elaboration. Preparation occurs at
the existing value-phase boundary, or immediately before a legacy type path that
still executes code. Recursive legacy definitions and unresolved body/constraint
paths retain preparation before evaluation. Checker registration and construction
validation are not removed. A zero-fuel regression proves duplicate static
declarations are diagnosed before executing a checker's value dependency.

This removes repeated scans and transient work when many checked type definitions
are pending. It does not yet separate all value inference from execution, statically
resolve recursive definitions, or replace checker registration with global graph
obligations. Full workspace validation passed (352 core, 41 CLI including language
acceptance, other workspace/doc tests), release build and source-size/diff checks
passed. Against `c45c55d`, checked-types-400 check time fell 43.97%, allocation
calls fell 63.80%, and peak heap fell 2.04%. Controls ranged from -1.55% to +0.52%;
this does not establish a general speedup. Details are in the measurement appendix.
