# RFC 0280: Session-Wide Type World and Execution-Free Inference

- Status: Revised accepted direction; implementation incomplete.
- Revision: 2026-09-10, making the session-wide typed IR and its completion gate
  explicit; independently solved module artifacts are transitional only.
- Tracking: [#175](https://github.com/hh9527/telora/issues/175).
- Branch: `feat/0280-arena-type-consumers`.
- Related: RFC 0276, RFC 0277, RFC 0279; performance investigation in #173.
- Evidence: [historical measurements](0280/measurements.md), including the
  regression at checkpoint `293098c`. No performance claim for this redesign.

## Decision and Scope

### Controlling implementation target

After resolving HIR for all reachable modules, construct one session-wide IR
with stable type slots for definitions, parameters, signatures, patterns and
expressions. This is the typed intermediate representation consumed by later
stages, analogous in purpose to a fully typed MIR. The implementation need not
introduce Rust MIR's control-flow or ownership machinery.

Allocate syntax-owned slots before constraint solving. Generic instantiation and
generated operations may allocate auxiliary slots during static solving; every
such operation must be typed before the execution gate. Cross-module references
connect to the defining record in this same graph, including when its slot is
still Unknown. A vector of independently solved module plans, or a shared store
that interns their exported descriptors, does not satisfy this requirement.

At convergence, resolve proxy chains and canonicalize structural type terms.
Every required syntax slot maps to a final session TypeId whose constructor and
argument edges are available in the type arena. Bound generic parameters remain
explicit, scoped type nodes; they are not unsolved inference variables. Recursive
nominal skeletons retain explicit identity edges. No Unknown, Conflicted, open
function-shape substitute or missing expression entry may cross the success gate.
Stable syntax/definition identity is distinct from canonical type identity:
unifying two type slots does not merge their source expressions or declarations.

The finalized IR owns type roots, basic type-definition skeletons, resolved
references and the call/constructor/trait evidence required for lowering. It
does not contain property-generated values. Static property contracts determine
presence; later execution computes the values associated with those contracts.
Code generation consumes this IR after inference scratch state is released,
without rebuilding descriptor trees to rediscover types or invoking inference.

At the execution boundary, import finalized type skeletons directly into the VM
as ordinary TypeDesc data. Maintain a TypeId-to-VM-value table for the same solved
graph and heap lifetime. Reserve recursive nominal identities before filling
their bodies; preserve sharing instead of rebuilding a tree for each consumer.
This is representation conversion, not inference or Telora evaluation. Compute
and associate property-generated values separately. Bound parameters describe
generic templates; concrete runtime witnesses require solved instantiations.
`--types-only` stops before this VM import entirely.

Finalized type skeletons and their TypeIds may enter the main world directly;
there is no per-module type-publication transaction. Data modules contribute the
fixed static contract `{ data: Value }` for JSON, TOML and YAML. Static solving
resolves their module identity but does not read or parse their contents, enforce
data-size limits or produce data-content diagnostics. Those checks belong to
subsequent loading. This also applies when a data module is the selected root.

The current per-module implementation does **not** meet this target. The
[whole-graph migration audit](0280/whole-graph-ir.md) records the concrete gaps
and replacement order. The checkpoints below are historical incremental work,
not evidence that the session-wide IR is already implemented. This target and
the acceptance criteria take precedence over earlier checkpoint next-step notes.

### Ordered phase contract

The symbol phase is complete when every module, export, import and local
symbol has stable session identity, and every symbol reference in all reachable
code has a settled resolve result: linked to a target, Unresolved, or Conflicted.
Unresolved and Conflicted are graph results carrying diagnostics, not failures
of the resolve procedure. They do not prevent proceeding to type solving.
Independent evidence still produces types; affected slots retain unavailable
or conflicting evidence. Gate execution and final session output after static
solving, rather than requiring an error-free symbol graph before type solving.
First inventory each module's exports globally;
then close lexical and imported references against that inventory. Both named
and wildcard imports use the same provider inventory; wildcard imports establish
search scopes. Source aliases retain their own authored identity and link to the
target declaration. Independent declarations must never merge because their
values or inferred types happen to be equal.

Stage conclusions are authoritative even when incomplete or conflicted. Type
solving must not rerun name resolution, select one ambiguous import candidate,
or repair an Unresolved reference using a later same-named environment entry.
Likewise execution and code generation consume type results without reinference.

`Conflicted(ConflictId)` indexes a separate evidence table. At minimum distinguish
`DuplicateDefinition`, retaining the distinct declaration IDs and locations,
from `AmbiguousImport`, retaining the referenced name and candidate source IDs.
Duplicate declarations are diagnosed even without a use. Overlapping wildcard
search scopes alone are not conflicts; ambiguity arises only at an actual use.
Nested lexical scopes have distinct identities; any additional language policy
on forbidden shadowing is separate from same-scope duplicate identity.

The type phase is complete when every required slot has a settled result:
Known with a normalized TypeId, Unknown, or Conflicted. First allocate all
required reference slots, then apply evidence to fill and refine them. Equality
evidence may merge type slots through proxies. Normalize and inspect all slots
without discarding independent facts because another slot is invalid. Only the
execution-ready typed artifact requires all necessary types and references to
be valid. Generic instantiations have distinct inference slots even when they
reference the same generic declaration.

These are two graph-closure phases with distinct identity semantics. Closure
means every reference has an explicit result, not that every result is valid.
Merely
indexing exports, retaining per-module solutions, or registering IDs without
closing the code's references does not complete either phase.

Implementation proceeds toward these two completion boundaries directly.
Intermediate branch states need not keep all functionality available or even
compile; maintaining each small migration as a working subsystem is not an
additional constraint. Do not add compatibility paths for that purpose.

0. Parse source modules into a module-name/ModuleId to HIR/CST inventory.
   A workspace name inventory does not require parsing every module body.
1. Resolve the reachable graph from the entry. Build each module's export-name
   index before typing; imports and lexical scopes resolve actual references to
   stable declaration identities. A wildcard import records a search scope,
   not bindings for every export. Declaration roles needed for pattern resolution
   come from source declarations, not executed values or solved interfaces.
   Type-dependent member selections become explicit constraints for phase 2.
2. Solve all type slots in one graph, normalize their TypeIds, and validate the
   complete typed IR. No VM or Telora execution capability is available here.
3. Create the VM, import finalized type skeletons, and parse/inject data modules.
   Data contents cannot feed back into type solving.
4. Generate property values and compute top-level values, possibly on demand
   according to their dependency graph. Module trait/property facts are static
   inputs regardless of whether an exported value name was referenced.
5. Dispatch execution through entry. Both execution phases consume finalized
   typing and cannot reopen inference.

`check --types-only` stops after phase 2. Ordinary check shares that same static
artifact before continuing with its existing tooling semantics. Stable source
IDs are session identities; cross-compilation stability is not required.

### Session ownership

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

The architecture has three phases:

1. Static solving requests module inventories, resolves the module graph and solves
   types for both tool code and runtime code. It has no VM or runtime-heap capability.
2. Tool execution consumes the solved program to compute metadata values and
   property values. Property presence was already established statically.
3. Runtime evaluation consumes the solved program and prepared metadata to execute
   user code, including the entry point.

Phases 2 and 3 use execution backends; neither may reopen type inference. Both can
later target a more efficient VM or native code without changing the static solver.
The immediate implementation priority is complete phase-1 type solving and removal
of inference from phases 2 and 3, while preserving their existing execution behavior.
Record-offset lowering and backend changes remain deferred. The first observable
entry to this boundary is now `telora check --types-only MODULE_ID`: discover and
solve the graph, including tool and runtime function bodies, without constructing
a VM or runtime heap. Ordinary `check` retains its execution behavior.
Both modes report `catalog_seconds` (workspace/catalog preparation) and
`check_seconds` (the selected checking path) in the JSON summary. In types-only
mode the latter measures static checking, including parsing and interface
publication. The difference between two separate runs is not an exact tool/runtime
phase measurement: the ordinary loader has not yet been replaced by a consumer
of this static solution, and its setup work differs.

The static entry now uses the same expression-publication gate as ordinary
analysis. This closes a verified gap where a heterogeneous branch result, such
as `if Bool.True { 1 } else { "wrong" }`, could pass the types-only entry when it
was not constrained by an exported signature. Discarding that expression does
not exempt it from checking. The solved TypeGraph is moved into the semantic
interface, preserving its IDs instead of rebuilding a graph from descriptors.
This is an initial downstream graph consumer, not yet a complete typed-program
handoff: open generic expression records still need a complete representation,
and the tool evaluator still has its own inference entry. Those must be removed
before claiming the full phase-1/phase-2 boundary is established.

The tool-expression inference body has now been extracted into
`solve_tool_expression_types`, which accepts static context, source information
and query control only. Runtime binding decoding was removed: imported types and
schemes come directly from static module interfaces. A direct test constructs no
VM or heap and verifies both type rejection and non-execution of `1 / 0` inside
a tool function. The current metadata scheduler still calls this pure solver;
moving that call into the complete session solution and consuming its evidence
later remains necessary. This extraction alone does not establish a single-pass
handoff or eliminate the existing handling of open tool-expression records.

Tool runtime-type evidence now stores `AnalysisTypeId` only. Failure to publish
a required runtime witness is an error before execution; there is no descriptor
fallback in that table or in the heap's batch graph-metadata entry. Expression
records use a Copy enum: a solved graph root, an open function shape (arity and
an independently solved optional owner ID), or an unresolved marker. The old
`Compatibility(TypeDescriptor)` tree has been removed. Open function shapes
provide lowering arity but cannot be converted to runtime type metadata or
claimed as final types. Completing the static typed-program handoff still
requires resolving or quantifying the remaining open expression records.

Tool owner preparation is now a separate VM-free `prepare_tool_execution` step.
It computes owner witness arguments, closes parameter indices, determines family
arities and records metadata roots before handing a `PreparedToolExpression` to
the evaluator. The evaluator consumes graph IDs and prepared owner evidence;
it no longer scans lexical scopes or substitutes owner types. Runtime witnesses
and owners share one graph-metadata batch, replacing per-owner descriptor
materialization. Open function shapes are also read directly from inference
slots, including proxy roots, without rebuilding a complete descriptor tree.
The metadata scheduler still owns the invocation sequence; moving this sequence
into the session-wide solved graph remains necessary.

`PreparedToolExpression` now owns compiled bytecode and its required binding
names. Compilation happens in the VM-free preparation step, before metadata
materialization. The evaluator accepts no source expression and does not call
the compiler: it materializes graph roots, links runtime values and runs the
bytecode. Required names come from the referenced static bindings, imported
pattern constructors and solved call/interpolation/owner evidence. A regression
test executes the plan with an evaluator that has no inference context and checks that a
missing runtime link returns an error rather than recompiling or panicking.
The compiler consumes the owned lowered tool expression, avoiding a second AST
clone at the preparation/compiler handoff. Tool solving no longer returns empty
evidence merely because the static context contains no nominal constructors:
scalar expressions go through the same inference and publication path, including
type-error checking, without evaluating their values. The former constructor
presence flag and its descriptor scans have been removed.
After compilation, plans discard runtime type-evidence roots absent from their
actual external links. Plans no longer own type graphs: the main-solver evidence
handoff, generated check expressions and property expressions publish directly
into the main module's type arena. That arena moves into tool preparation,
temporarily into the evaluator for execution, and back into the returned Analysis.
Compilation, runtime roots and module analysis retain the same IDs, without Arc,
copying the graph, merging graphs or remapping IDs. The other inference-context
resources are dropped before execution. Unification across the whole session
remains necessary.
Main expressions and tool bindings now share one inference-slot publication table
against that same arena. The tool handoff reuses already published IDs instead of
allocating another table and traversing the main solver's slots again. The table
is reused only while the solver is frozen and dropped before independently
generated expressions are solved. Per-binding evidence selection still scans
module maps, and generated expressions still have separate static solvers.
The follow-up [interface and descriptor investigation](0280/session-interface-investigation.md)
finds that graph-to-descriptor conversion accounts for 23.40% of allocation calls
in the current ontology profile, including 8.82% under local annotation collection.
The next migration should keep declaration/annotation graph IDs through solver
ingress, then move interfaces into the session table. Changing import selection
alone leaves both upstream artifact copies and the larger descriptor boundary.
These are inclusive allocation counts, not expected runtime savings.
The evaluator now retains a flat graph-ID-to-metadata table for the lifetime of
that immutable tool graph and work heap. Repeated consumers use completed slots
instead of allocating a full conversion table and rebuilding metadata per call.
Empty batches allocate nothing. Materialization failure clears the table so
provisional recursive owners cannot be mistaken for completed metadata on retry;
this is runtime recovery, not a type-solving fallback.
The ordinary analyzer no longer records a parallel tree of provisional types for
every expression. Preliminary binding projection returns only its root type;
annotations and the module result are not separately traversed for provisional
records. Expression publication consumes the main solver's graph, with nominal
descriptor bridges constructed from its completed evidence only. Import path
literals explicitly enter that graph as String, closing a semantic-coverage gap
previously hidden by provisional records. Property contract projection also no
longer builds and immediately discards a recording map. Removing the remaining
preliminary binding projection requires moving its contract/environment consumers
onto the shared solver artifact.
The preliminary pass now skips body projection for `impl` and all `def` bindings.
Established contracts remain static inputs; definitions without a contract are
filled by the main solver. Complete program inference checks their bodies.
Root projection no longer retains an expression-recording callback or traverses
operands whose types cannot affect its result. Known function results do not
require argument projection; literal string/comparison result types, return/fail,
index result types, and similar syntax avoid discarded child projections.
Ascription projects the value only when the target does not provide a type.
Full operand and body checking remains the complete program solver's job.
Closure projection stops as soon as a parameter lacks a projected type. Its
result is necessarily unknown in that situation, so traversing the body cannot
improve this preliminary result. Complete closure inference still runs in the
main solver; this does not suppress its constraints or diagnostics.
Uncontracted definitions now use the main solver's slots without preliminary
body projection. Provider aliases and factories obtain their callable contracts
from that same solver before property-dependent evidence is finalized. The earlier
attempt to remove projection before migrating this dependency was withdrawn;
this implementation changes the contract discovery ordering first. Preliminary
`let` projection remains, including diagnostics whose data/rule provenance still
needs transfer to the main solver's conflict evidence.
Explicit trait calls now infer their signature from the trait declaration and
enqueue an implementation-evidence obligation in `pending_type_constraints`.
They no longer select an implementation before checking the remaining arguments:
`Combine.combine(x, 1)` can determine an initially unknown Self from the second
argument. Evidence destinations distinguish hidden call arguments from trait
member dictionaries. String interpolation also records an obligation instead of
selecting Display evidence while inferring its expression. Both preserve lexical
evidence until final resolution; unresolved trait member targets are diagnosed.
Property-dependent candidate selection now runs during evidence finalization.
Provider result slots now refine property-presence facts before that gate, within
the same program solver. Provider expressions are inferred against completed
binding slots; their normalized return types determine the nominal property.
Local evidence roots are allocated from the module ID and contract ordinal,
without a VM, runtime heap or runtime type ID. Tool execution fills these exact
roots and rejects a materialized property set inconsistent with the solved
contracts. This does not retry whole-program inference. Generated property/check
plans still have separate static solvers and need to consume the final artifact.
The main solver now records provider contracts by decorator location for type,
field and variant decorators. Tool planning and the remaining types-only decorator
validation require those records; neither reprojects provider types nor reinfers
their configuration expressions to discover a contract. Missing records are an
error, with no projection fallback. The old decorator projection function is
deleted. Member contracts are checked before the same evidence-finalization gate,
while only type decorators establish type-level property-presence facts.
Synthetic chained calls still run through `prepare_property_call` and its static
solver; their compilation has not yet migrated to consume main-solver evidence.
Property decorator calls now use `prepare_property_call` to solve once against
the declared property result and compile a plan without VM/heap access. The
previous lightweight recorded inference and descriptor-table supplementation
have been removed from this call path. Scoped static inputs are restored before
execution; only then is the chained previous runtime value materialized. Provider
signature discovery and capability validation still need to move into the final
session-wide static artifact; this is not yet a fully static property scheduler.
Remaining tool-expression consumers no longer supplement solved evidence with
the earlier lightweight inference descriptor table. Tool plan types now come
only from the tool solver's graph publication. Check-dependency annotations use
the shared static annotation scope directly, replacing evaluation followed by
runtime metadata decoding; invalid static annotations are reported rather than
silently falling back to inference without the annotation. This removes that
execution dependency, while the scheduler's broader retry loop remains to be
replaced by the session-wide solved artifact.
Ordinary type declaration scheduling now includes every type definition. The
special source-order path for types depending on value helpers is removed, as
are VM fallbacks for ordinary declarations, type families and concrete recursive
components. Missing static bodies produce diagnostics. `materialize_type_body`
requires a graph ID; recursive descriptors read the solved graph without runtime
values or evaluator access. Unused runtime declaration wrappers and the retained
reverse dependency table have been deleted. This closes declaration-body
execution fallbacks. Declaration materialization and tool execution now consume
prepared plans after per-module solving, and the execution object cannot own an
inference context. Moving ordinary module loading and these consumers behind
the complete session static artifact remains required.
Invalid `Unchecked` targets now record their precise diagnostic during static
elaboration. Previously that error relied on the declaration execution fallback;
removing the fallback initially exposed only an unknown-type diagnostic in
three language acceptance fixtures. The static diagnostic closes that gap.
Recursive family signatures are now prepared before runtime object construction.
The VM-free preparation validates the solved body and bound parameters, builds
the TypeScheme and determines whether runtime rebuilding is required. The
materializer consumes these static parameters and graph roots; it no longer
parses binders again or returns a newly inferred/published scheme. Static family
inventory also no longer accepts the runtime family-template table: its former
local-table input was empty at every call site, while local solved schemes are
registered explicitly during solving. This removes a misleading runtime input
from static preparation without adding a replacement fallback.
Concrete nominal identities now enter the graph in VM-free
`prepare_static_declaration`; runtime materialization receives an immutable graph
and a solved root. `intern_declared_body` wraps the existing body ID directly,
removing the previous body-graph to descriptor-tree to graph round trip. The
declared-identity index is maintained, and reserved Never-body identities refine
the same row. Descriptor conversion remains at the current runtime boundary;
metadata construction is not yet deferred until the whole session is solved.
Declaration scheduling now emits `DeclarationPlan` records for concrete types,
families and recursive components, without creating their runtime metadata.
After the declaration loop succeeds, `materialize_declarations` consumes the
immutable graph and ordered plans, reserves placeholders and builds runtime
objects. Recursive reservations use flat vectors rather than per-component maps.
The tool inference context is created from completed declaration
inputs; the former initial clone plus immediate refresh and per-declaration
publication are removed. This establishes a declaration-level handoff, not the
final session handoff: native/bootstrap setup still precedes it. Function-body
inference and tool-plan preparation now precede materialization; tool execution
follows it.
The materialization boundary also follows definition-contract preparation,
trait-overlap validation, interpreter-contract checks and unresolved-reference
validation. These checks require no runtime declaration values. The tool context
is therefore created once from the completed contract environment, avoiding a
second whole-environment refresh and avoiding local metadata creation on these
static errors.
Property presence collection is shared between ordinary and types-only checking
through VM-free `solve_declared_property_contracts`. It reads provider result types
from the main solver and target types from its environment, validates concrete nominal properties
and deduplicates by static type identity, including repeated declarations. It
does not read target runtime values. Static evidence binding names are shared by
both check paths. The ordinary execution consumer uses canonical runtime type IDs
only to locate the materialized values for the already assigned evidence roots.
Property value reduction remains an execution concern.
Native type signatures are now installed during their initial import, before
declaration solving. The later binding pass links native values without decoding
the same type metadata and reconstructing its scheme a second time. Initial
native descriptor import still comes from the host runtime root; replacing that
input with the loader's static native inventory remains required.
The preliminary binding pass now updates static inputs and records ordered tool
tasks without evaluating them. Declaration materialization, construction-check
factories and tool dependency execution follow `solve_program_types` and
`publish_program_expressions`. The obsolete per-binding tool-context publication
and whole-environment refresh methods are removed. A regression proves a later
function-body type error is reported before an earlier failing construction-check
factory can execute; correcting the type error permits that factory to run.
This is a per-module ordering improvement, not completion of the session-wide
boundary: imported module execution remains in the ordinary loader, tool
expressions still perform their own inference, and open generic expression
publication and post-solve normalization still need the final typed artifact.
The per-module execution boundary now also follows inferred-scheme normalization,
interface type publication and publishable-scheme validation. Tool setup consumes
the completed binding types and quantified schemes instead of only the earlier
lightweight environment. Expected types for queued tool tasks are normalized
before any declaration materialization. Independent tool-expression inference
still exists and remains a required removal; providing it solved inputs is not
equivalent to consuming a final compiled static artifact.
The normal top-level tool-task path now uses `prepare_solved_tool_binding` to
publish the main solver's expression records and select its constructor, call,
trait/interpolation and lexical-parameter evidence. Plans are compiled before
declaration materialization; execution accepts those plans without invoking the
independent tool solver. Missing static evidence is an error, with no reinference
fallback. The old typed-tool evaluation wrapper is removed. Generated
property/check expressions still use separate preparation/inference and must be
migrated to the shared static artifact.
Construction-check dependency retries now borrow the same precompiled top-level
plans as normal tool execution. They no longer infer, elaborate annotations or
compile dependencies per retry. Missing required plans report a static-plan error,
without a fallback. Execution borrows bytecode and the plan graph, allowing
relinking/retry without cloning them; missing external bindings are checked
before metadata materialization. Each top-level task now retains its successfully
computed value in a definition-local slot. The normal task loop consumes that
value when construction dependency scheduling has already executed the definition;
failed attempts leave the slot empty for later retry. Completion is independent of
the name-indexed runtime linking environment. Construction-check contracts now
prepare once through a VM-free function before declaration materialization and
tool execution. The execution scheduler receives compiled plans and parameter
IDs, and retries only linking/execution; invalid contracts are diagnosed during
preparation, without an execution-time inference fallback. Their static plan
preparation still uses an independent expression solve rather than main-solver
evidence. Property capability expressions and generated provider calls now also
prepare before declaration materialization/tool execution. Prepared provider
plans retain their static nominal result descriptor, avoiding repeated contract
elaboration during reduce. Runtime capability evaluation, capability validation
and chained value reduction keep their execution semantics. `ToolEvaluator` no
longer contains an inference context, the evaluator-based inference wrapper is
deleted, and the temporary tool inference context hands off only its completed
type graph before execution.
Property plans currently use source locations to link execution sites to static
plans; shared graph/plan IDs and removal of independent generated-expression
solves remain necessary. This per-module boundary does not yet complete the
session-wide static artifact or remove ordinary imported-module execution.
For statically known records, solving fixes field identity, order and type. Lowering
records the selected field/layout identity, and the backend translates that to a
slot or byte offset for the target representation. Field access then uses that
offset instead of resolving a field name or guessing receiver types during execution.

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

The session owns syntax/HIR/type arenas; stages retain IDs and borrow these stores.
Sharing a node across stages does not justify an Arc on each node or module.
Tool inference now borrows the analysis-owned HIR without reference counting.
An independently lived, immutable cross-session snapshot may have a single
outer shared owner when its consumers require it. Prepared modules are now owned
directly by module rows, and the module graph cannot be cloned. Loader dependency
preparation is separate from compilation: recursive loading carries only import
operands and a binding cursor, then compilation borrows session syntax again.
Recovery likewise releases syntax borrows before loading dependencies and borrows
again for analysis. No syntax node is removed during recursion. This ownership
change does not yet remove legacy dependency execution or flatten AST/HIR nodes.
Semantic snapshot projection borrows input facts rather than first cloning their
HIR and type graphs. The resulting snapshot still owns projected records with
remapped IDs; this remains a boundary to migrate toward global ID consumption.

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
VM or heap metadata. There is no execution fallback for type syntax. Migrated
consumers must expose unsupported static cases as failures rather than execute a
legacy compatibility path; this keeps missing capabilities visible to tests.
Unmigrated consumers remain explicit outstanding work and cannot justify calling
the phase boundary complete.

### 3. Session-wide inference and resolution

The phase boundary is a hard contract: no Telora code executes before all required
type solving completes. After that boundary, lowering, code generation and value
execution consume the solved graph and recorded evidence; they do not launch new
type inference or guess types from runtime values. Generic type arguments are
explicit evidence parameters, and dynamic checks use explicit witnesses. Removing
both runtime-produced types and late tool-expression re-inference is required for
completion; a fast fallback that violates either side is still unfinished migration.

Enforce this boundary through capabilities and dependencies. The type-solving
context contains HIR/module identities, the type arena, constraints, static
property-presence evidence and diagnostics. Its API must not accept a VM, evaluator,
runtime heap, executable values or callbacks that grant access to execution.
Runtime metadata materialization belongs to the later consumer, not the solver.
The execution layer receives finalized types and evidence after the phase gate;
it must not call back into inference. Merely avoiding VM calls while an enclosing
analysis function still owns an evaluator does not meet this architectural gate.

Diagnostics follow the slot solver directly. When a representative slot first
becomes Conflicted, record one diagnostic with both conflicting evidence locations
while they are available; keep solving unrelated slots. After resolving proxies,
scan the slot array and report remaining required Unknown slots. Do not report the
same conflict again through each proxy or reconstruct its evidence chain afterward.
Any required unresolved/conflicted fact prevents code generation and execution.
Multi-error reporting needs no separate diagnostic scheduler and must not depend
on executing Telora or repeating inference in a recovery world.

Type solving has no failure-and-recovery semantics: Known, Conflicted and the
Unknown slots remaining at convergence are all solver results. A conflict does
not abort the graph solve, and an unresolved fact never triggers evaluation or
a second inference world. Failure and recovery belong to the subsequent value
execution phase. The final type-phase gate inspects the solver result and its
diagnostics before admitting that phase.

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

For type-level properties, the static relation is
`(TypeId, PropertyTypeId) -> Bool`: declarations and provider return types establish
presence without executing the provider. While the relevant declaration graph is
incomplete, presence remains unresolved rather than defaulting to false; conflicting
facts remain errors. `HasProperty` consumes the resolved presence fact, independently
of whether the property's value has been computed.

The value phase fills `(TypeId, PropertyTypeId) -> value: PropertyType` for present
properties. It executes the declaration chain's ordered reduce and exposes one
final value per key. Consumers query that final record, not a list of competing
declarations. Failed or pending value/applicability work blocks successful final
output, without requiring provider execution to establish static presence. This
is the target phase separation; existing eager paths still require migration.

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
The accepted direction is implemented locally; commit/push follows the user's
authorization and is not a prerequisite for investigating or implementing it.

The [static result handoff audit](0280/static-phase-handoff.md) identifies the
remaining input contracts, discarded evidence and execution dependencies between
the pure and ordinary loaders. Its per-module handoff proposals are transitional;
use the whole-graph migration audit for the architectural replacement order.
Calling the pure checker before the current ordinary loader
is not an implementation of the handoff: it would repeat program solving.

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

## Type-only check and phase measurement

`check --types-only MODULE_ID` is now available as an intermediate observation
point for the static phase. Its loader owns syntax and static interfaces without
a VM or runtime heap. It checks function bodies and static tool contracts but
does not execute properties, `@check`, or module values. Ordinary `check` retains
its existing full-check behavior.

Both modes emit `catalog_seconds` and `check_seconds` in their JSON summary.
The static and ordinary loaders are not yet one shared pipeline, so subtracting
these measurements does not give exact metadata/runtime phase time. Unifying
the ordinary path behind complete static solving remains required; the new flag
does not satisfy that architecture gate by itself.

## Acceptance

### Architecture and ownership

- Import required finalized skeletons as VM TypeDesc data without inference or
  Telora execution. Repeated roots reuse the same imported value, recursive
  nominal bodies preserve identity, and importing skeletons does not evaluate
  property providers. Keep graph/heap lifetime identity explicit in the mapping.
- Before solving, enumerate syntax-owned slots across all reachable HIR modules.
  Check exact coverage for definitions, parameters, signatures, patterns and
  expressions, including nested closures and tool bodies. Generated static
  operations and generic instances receive additional slots before finalization.
- A cross-module diamond fixture connects imports/aliases/re-exports to the same
  definition slot before its type is known. Later constraints resolve that slot
  without importing descriptors, solving a second copy or remapping module graphs.
- Finalization checks all required slots, not only entries inference happened to
  record. Tests cover multiple independent conflicts, remaining Unknowns and
  valid bound generics. Every successful syntax record has a final TypeId.
- Release solver scratch state and generate tool/runtime bytecode from the same
  finalized session IR. Both check modes use this static result; neither performs
  a second module solve. No open-shape or descriptor fallback is permitted.
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

### Static recursive concrete definitions

Concrete nominal recursive components now reserve every declaration identity in
the analysis graph before elaborating any body. Each supported body resolves
against those identities; the completed body IDs are then written into the
reserved nominal rows. Filling a row does not reconstruct descriptors or change
nominal hashing. Self-recursive and mutually recursive declarations have a
zero-execution-fuel regression test.

Runtime metadata reservation/sealing remain compatibility consumers. Supported
recursive declarations now retain both owner and body IDs: initializer shape
validation reads the solved body, and signature publication/canonicalization reads
the solved owner. Neither step decodes the generated runtime metadata back into
a separate graph. Descriptor-based signature consumers still remain, so this is
not yet the final graph-only downstream path. Unsupported recursive bodies
retain the original evaluation/validation order, and recursive generic declarations
still use the legacy family builder. The new recursive-types benchmark isolates
400 self-recursive concrete declarations without constructing user values.

Workspace validation passed (353 core, 41 CLI and other workspace/doc tests);
the final legacy-order adjustment also passed all 353 core tests and release build.
Against `d99f2d5`, recursive-types-400 check time decreased 34.44%, allocation
calls decreased 27.79%, and peak heap decreased 3.43%. Control movements were
small (-0.13% to -1.53%); full evidence and limitations are in the appendix.

### Static self-recursive family templates

Unconstrained self-recursive nominal families now reserve an owner with symbolic
argument identity in the analysis graph, elaborate the body, and fill that row.
While the template is pending, self application is accepted only with unchanged
bound parameters in declaration order, preserving the existing language rule.
The temporary self binding is removed after elaboration; no type environment is
cloned for this step. Body validation and template signature read the solved graph.

Runtime family closure creation remains a compatibility boundary. At that boundary,
self-reference metadata reuses the reserved symbolic root; creating another object
with the same nominal identity caused a duplicate-sealing regression in the existing
checked-recursive-types dyn projection test, and was corrected. Zero-fuel coverage
includes concrete applications of recursive templates, distinct Int/String and
phantom identities; changed or reordered recursive arguments remain rejected.
Constrained recursive families and recovery analysis still use the legacy builder.

Final full workspace validation passed (355 core, 41 CLI including language
acceptance, and other workspace/doc tests), release and source-size/diff checks
passed. Compared with the immediately preceding `d28f6f8` checkpoint, not the
original RFC baseline, recursive-families-400 check time decreased 23.91%,
allocation calls decreased 11.68%, and peak heap decreased 2.07%. Controls moved
between -0.71% and +0.61%; this does not establish a general speedup.

### Constrained family shapes and deferred obligations

The static family registry now admits structurally supported templates with
constraints. It elaborates their shapes; the original TypeSchemes retain the
constraints for final inference. Type declaration bodies already pass through
that inference boundary. Declaration contracts now additionally collect their
constrained family applications and check them with the declaration's lexical
evidence, without re-inferring ordinary signature structure as runtime data.

Investigation exposed an existing gap: a missing Property evidence application
in a type body was rejected, while the same application inside a function contract
could be accepted after legacy tool inference errors were discarded. That contract
case now fails. Re-inferring entire contracts as ordinary value expressions was
rejected during validation because it violated the type/metadata boundary; the
application-only obligation traversal preserves that distinction. Regressions
cover missing property/trait evidence and zero-fuel generic applications carrying
lexical evidence. Unsupported templates, recursive bounded templates and recovery
remain migration work. Obligation targets still use the existing inference and
descriptor adapters; this is not a claim of a completed session-wide ID pipeline.

Full workspace validation passed (358 core, 41 CLI including language acceptance,
and other workspace/doc tests), release and diff/source-size checks passed.
Against `2c426a7`, local/qualified family-obligations-400 check times decreased
28.49%/28.62%; controls moved between -0.60% and +0.73%. Qualified allocations
decreased 18.34%, but peak heap increased 2.68% (16.81 -> 17.26 MB). The appendix
records this tradeoff, cumulative measurements, scope and artifacts.

### Session-owned module syntax without reference counting

PreparedModule is now stored directly in the ModuleId-indexed row, including
failed parse and recovery facts. ModuleGraph and ModuleSkeleton no longer
implement Clone. Undiscovered direct inputs retain their existing compatibility
storage with the same exclusive ownership.

Strict loading prepares dependencies separately, retaining only an import's
operands and the next binding index across recursive loading. It then borrows the
original Program for analysis and compilation. Recovery releases its syntax borrow
before dependency traversal and reacquires it for strict/partial analysis. Source
records remain present throughout recursion; no temporary removal, AST clone,
unsafe reference or replacement reference counter is used to cross this boundary.

Full workspace tests passed (358 core, 41 CLI including language acceptance, and
all remaining workspace/doc tests), as did release and diff/source-size checks.
The existing source-change regression verifies the original ModuleId/SourceId and
successful compilation from discovered syntax after both backing files change.
This step removes per-module Arc, but HIR shared ownership, legacy dependency
execution and downstream descriptor materialization remain separate migration work.

### Borrowed HIR and semantic projection inputs

Strict and recovery inference now own their HIR directly. ToolInferenceContext
borrows that HIR for the duration of inference; final Analysis/PartialAnalysis
receives the same object by move. No Arc construction, clone or try_unwrap remains
in this handoff. HIR is still resolved per analysis, not yet stored in a unified
session HIR arena.

WorkspaceSnapshot projection now accepts borrowed input records and sorts a vector
of references. Strict loading, run/eval and test no longer clone complete semantic
inputs, HIR and type graphs merely to project them. Existing consuming callers use
the same borrowed implementation while retaining ownership until projection ends.
Synthetic builtin records are owned locally until projection completes. The final
snapshot owns its output records and remains usable independently of those inputs.

Snapshot type/definition graph remapping and the Analysis copy between compiled
module and semantic input remain. This is not yet direct consumption of session
IDs. The benchmark runner has a test mode that imports each existing workload and
executes one trivial successful test, specifically to exercise the former
clone-before-projection path; ordinary check uses the consuming path as a control.

Full workspace passed 358 core, 41 CLI including language acceptance, and remaining
workspace/doc tests; release and diff/source-size checks passed. Against `64d4e68`,
test diamond-400 median decreased 3.44%, allocations decreased 3.00%, and peak heap
decreased 21.28% (32.99 -> 25.97 MB). Test property-400 allocations decreased 1.16%
while peak heap stayed at 29.26 MB. Check controls showed no clear timing gain.
These command-specific results and their limitations are in the measurement appendix.

### Linear type projection and shared named roots

Semantic type projection now scans each source arena once into a contiguous output
span. A per-module base/length record replaces per-type remapping arrays; child IDs
are translated by addition. Forward edges and nominal cycles do not trigger
recursive traversal or Pending-slot backfilling. Output order follows source arena
order rather than DFS discovery order. Snapshot IDs remain local to the snapshot;
names, expression/definition facts and result types all use the same translation.

A regression for recursive edge preservation exposed pre-existing duplicate
nominal rows: name publication reserved a row and copied an already solved nominal
node into it. It now reuses the solved nominal ID. Where forward names still need
reservation, the placeholder becomes a Ref edge instead of a copied constructor;
the final name table points at the resulting roots. Temporary reservation records
are a vector of IDs, not another map of cloned names. This preserves pre-existing
forward references while avoiding duplicate nominal identity rows during handoff.

Regressions cover shared function parameter/result roots, recursive back edges,
independent output spans and forward names resolving to shared primitive roots.
This is still projection into a separate public graph, not the final global-ID
consumer model; unresolved aliases may retain proxy rows and descriptor adapters
remain. Current CLI check execution behavior is unchanged.

Workspace passed 360 core, 41 CLI including language acceptance and all remaining
tests. After the final temporary-map-to-vector adjustment, all 360 core tests and
release passed again; diff/source-size checks passed. Compared with `d5dbf63`,
check timing moved between -1.83% and +0.08%, and test between -0.21% and +0.20%.
Measured peak heap was essentially unchanged. These small movements do not prove
a broad speedup; full scope and artifacts are in the measurement appendix.

### Tool runtime type arguments consume graph roots

The tool-expression runtime-type binding path now builds no-origin metadata
directly from TypeGraph roots, without first reconstructing a TypeDescriptor tree.
The builder records produced values by source node ID, reserves nominal owners
before following their bodies, and preserves reuse of sealed nominal metadata.
Structural revisits across a nominal boundary are valid; a structural cycle with
no nominal boundary remains rejected. Per-root construction scratch belongs to the
materialization operation, not a new persistent stage cache.

Type-argument family arity is read from graph nodes and nominal identity arguments,
including phantom bound parameters. Open compatibility roots still use their
descriptor path. Origin-bearing construction and owner-evidence substitution keep
their existing adapters; nominal argument identities also retain a descriptor
boundary. This does not change property/check execution or CLI check semantics.

Regressions compare direct graph metadata with descriptor-built metadata, include
symbolic phantom arguments, and cover a structural root entering a nominal cycle
as well as rejection of a purely structural self-cycle. A tool-type-arguments
benchmark uses generic calls with a repeated-element tuple inside distinct
property providers to exercise runtime type-argument construction.

Full workspace passed 362 core, 41 CLI including language acceptance and all
remaining tests; release and diff/source-size checks passed. Against `5320e42`,
the tool-type-arguments-400 benchmark median decreased 1.96%, allocations decreased
0.75%, and peak heap decreased 7.16% (49.55 -> 46.00 MB). A heaptrack backtrace
confirms the new builder is exercised. Timing changes are small and do not prove
a general speedup; scope and artifacts are recorded in the measurement appendix.

### Batched tool metadata roots

Runtime type bindings for one tool expression now share one graph-to-value
construction table across all roots. Repeated roots and shared children reuse
metadata values by graph node ID. The table is initialized on the first graph
root and discarded at the operation boundary; descriptor-only batches retain
their compatibility path without graph scratch. This advances downstream ID
consumption without introducing a persistent stage cache.

Metadata is built in binding order, then family wrappers and local bindings are
created. This preserves user-code evaluation behavior. Regression coverage checks
mixed descriptor/graph ordering, identical repeated-root values, empty batches,
and successful construction after a failed batch discards its scratch. The
tool-shared-arguments workload extends the single-call provider case to eight
generic calls per provider, exposing repeated roots within one tool expression.

Full workspace passed 363 core, 41 CLI and all remaining tests; release and
diff/source-size checks passed. Against `27835c3`, shared-arguments timing moved
-0.59%, allocation calls -1.04%, and peak heap -0.91%. This is a small allocation
reduction, not evidence of a material speedup; details are in the measurement appendix.

### Session-owned strict module analysis

Strict compilation now moves Analysis into the session semantic input table.
CompiledTeloraModule retains its module key and borrows that analysis for dependency
execution and entry/eval contract validation. After snapshot projection, selected
modules transfer their analysis into LoadedModule. This removes the full Analysis
clone at compilation, including its HIR, type graph and inference evidence, without
adding Arc ownership or temporarily removing facts during dependency recursion.

Dependency artifacts still clone their required ModuleInterface/result scheme.
The table retains its existing string module keys; this is an ownership migration,
not the final global ModuleId/TypeId consumer representation. Recovery and the
independent output snapshot projection retain their existing paths. User-code and
property execution timing is unchanged.

Full workspace passed 363 core, 41 CLI and remaining tests; release and
diff/source-size checks passed. The runner now has an eval wrapper that exercises
strict loading, confirmed by heaptrack; test mode uses WorkspaceBuilder and serves
only as a control here. Against `7d3be57`, eval diamond-400 timing decreased 3.19%
and allocation calls 2.73%; property-400 decreased 1.13% and 1.08% respectively.
Peak heap did not improve meaningfully (diamond increased 0.33 MB). Details and
control results are preserved in the measurement appendix.

### Borrowed compiler evidence across closures

The LIR compiler now borrows the inferred constructor and declared-owner evidence
maps. Program compilation borrows Analysis; tool-expression compilation borrows
the maps owned by that operation. Every nested closure receives the same references
instead of cloning both complete maps. Their lifetimes are separate from temporary
capture/type-slot inputs, so register and mutable lexical state remain local to
each compiler. Generated LIR and bytecode retain no borrowed analysis references.

This removes per-closure copying of solved facts without introducing Arc. Evidence
is still indexed by source Location, and per-owner lookup/capture discovery plus
other compiler-owned tables remain; this is not the final session-ID lowering API.

Full workspace passed 363 core, 41 CLI and remaining tests; release and
diff/source-size checks passed. Against `1b04e09`, property-400 timing decreased
4.67% under eval and 4.61% under check. Eval/property-400 allocation calls fell
9.18%, with unchanged peak heap. A filtered heaptrack stack confirms removal of
the baseline's 160,400 string allocations from nested owner-table cloning.
Controls and timing variability are recorded in the measurement appendix.

### Range-indexed owner capture facts

Each compilation entry builds one sorted vector of borrowed owner-evidence entries,
ordered by SourceId/start/end. Nested compilers borrow the same index. Hidden-owner
capture discovery binary-searches the start and scans only entries starting within
the closure's source range, filtering entries whose end escapes that range. The
old implementation scanned the whole module map for every closure and compared
offsets without SourceId. Cross-source equal offsets no longer create captures.

The index neither clones evidence nor survives compilation. Captured-name ordering,
generic hidden-parameter filtering and local register allocation remain unchanged.
This reduces range lookup from a full-table scan per closure to O(log N + K), after
one O(N log N) sort; nested scopes still inspect their contained evidence. A focused
test covers nested/overlapping scopes, equal boundary offsets, different sources,
missing ranges and an empty index. This remains a Location-based adapter pending
the final session definition/expression-ID consumer representation.

Full workspace passed 364 core, 41 CLI and remaining tests; release and
diff/source-size checks passed. Incremental eval/property timing ranged from
-1.18% (100 providers) to +0.72% (400), with peak heap unchanged and 449 additional
allocations for the 400 case. The actual ontology check on asset revision `1a871a0`
passed using absolute -C and measured 2.99214 -> 3.01979 seconds (+0.92%) against
`ecb5008`. No measured speedup is claimed for this checkpoint. Full measurements,
including the relative-path pre-analysis failure, are in the appendix.

### Static annotations and recursive partial analysis (in progress)

The annotation walker now elaborates into the inference publication's type graph
without evaluator, heap or runtime-binding inputs. Primitive, generic, nominal
and applied-family local annotations pass with zero execution fuel. Selected
imports resolve through HIR and module type interfaces, including renamed exports;
an imported metadata value alone does not establish a type declaration. The
recursive semantic Value/codec integration test passes with this static lookup.

Partial analysis now elaborates nonrecursive definitions, fills concrete recursive
components directly in the graph, and elaborates self-recursive family templates
before materialization. It has no calls to execute type bodies or decode their
results. Its family table is built once and extended as definitions become known.
Concrete, applied and recursive type tests pass with zero fuel. Metadata adapters
for remaining value consumers are still present.

Static elaboration records family arity and recursive-argument contradictions in
the graph, including application and declaration locations. Collection visits all
sibling fields/arguments even after encountering an unresolved or conflicted edge.
Partial analysis publishes these contradictions as Conflicted facts and reports
remaining root Unknown facts after convergence, without duplicating diagnostics
on their dependents. A zero-fuel test verifies two independent recursive argument
conflicts and a separately solved concrete type. The recursive family builder now
requires a solved graph and has no execution/decode fallback.

Full workspace validation passed: 367 core tests, 41 CLI tests including the
language acceptance suite, and the remaining test groups. Source-size and diff
checks and release build passed. Against `b3ce28b`, the real ontology check median
fell from 2.996063 to 2.053956 seconds (-31.44%; ten samples per version, reversed
orders). Separate heaptrack runs measured 22.21% fewer allocations; peak heap was
essentially unchanged (232.88 to 232.63 MB). Check retains value evaluation; this
is not the later type-only check optimization. Details are in the measurement appendix.

The no-failure/recovery solver model above is not yet implemented end to end:
unification and the main analysis entry still return early errors, and static
elaboration can still enter legacy evaluation at other main-analysis call sites.
These must migrate to graph results and a final phase gate; this checkpoint does
not establish the execution-free session boundary.

### Static declaration signatures and constraints

Declaration signatures now require static graph roots, and both declarations and
type families consume static constraint results. The legacy contract evaluator
and runtime constraint evaluator have been deleted. Constraint solving retains
known constraints alongside unresolved constraints and their locations, rather
than stopping at the first missing trait. Unknown declaration type names are
diagnosed from HIR's unresolved references at the type-result gate.

Full workspace validation passed: 368 core, 41 CLI including language acceptance,
and remaining test groups. Diff and source-size checks passed. These follow-up
changes have not received a new release performance comparison; the measurements
above apply to the preserved static-annotations binary. Main analysis still
returns the first blocking diagnostic and owns ToolEvaluator, so neither global
multi-diagnostic output nor the no-execution-capability boundary is complete.

### Partial solver without runtime resources

Partial type solving now has a separate entry in `types/partial-solver.rs` with
no VM, evaluator or runtime-heap parameter. The existing entry adapts external
inputs to type descriptions before invoking it, using an immutable heap borrow.
The solver constructs recursive family schemes directly in the graph and no
longer installs bootstrap values, allocates placeholder metadata, creates runtime
family closures or materializes recursive owners. ToolInferenceContext is no
longer part of partial solving. Cancellation remains a query-control checkpoint.

A direct solver test supplies source and static inputs, with no runtime resources,
and verifies a recursive family, its concrete application and an unresolved-name
diagnostic. Full workspace tests passed (369 core, 41 CLI including language
acceptance, and remaining groups); diff/source-size checks passed. No new release
performance claim applies to this follow-up. Main analysis and late tool inference
remain the next capability-boundary migration; the full first phase is incomplete.
