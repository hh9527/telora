# Whole-graph typed IR migration audit

Date: 2026-09-10. This is an implementation audit, not a performance result.
The controlling target is RFC 0280's session-wide typed IR.

## Current gaps

| Boundary | Current evidence | Required replacement |
| --- | --- | --- |
| Module scheduling | `module/type-check.rs::StaticWorkspace::solve` recursively solves dependencies and selects descriptor interfaces using pre-resolved module/export-row targets. | Connect those targets to session definition/type-slot IDs and schedule constraints over shared records. |
| Slot ownership | `types/inference-context.rs::GenericInference::new` takes one HIR program and starts with an empty expression `records` map. `record_type` creates/replaces expression edges while inferring. | Preallocate syntax-owned slots in the session graph, with explicit evidence for contextual conversions. |
| HIR identity | Both checking modes, module loading, generated entry loading and native installation consume HIR from `module/static-names.rs`. Local IDs are qualified by their session ModuleId; bootstrap and Host symbols also have explicit IDs. | Allocate type slots against these source identities without rebuilding HIR. |
| Static handoff | `types/solved-module.rs::SolvedModulePlan` owns one module's arena/HIR/evidence. `types/type-check.rs::check_module_types` retains only interface and types. | One typed program retains every required syntax type and lowering fact; modules are ranges/namespaces within it. |
| Completion | `types/inference-publication.rs::publish_program_expressions` visits recorded locations, rejects some failures, but omits other unsuccessful publications. | Full required-slot scan, conflict provenance and Unknown diagnostics; a success artifact cannot contain missing types. |
| Consumer boundary | Ordinary analysis calls solve then execute per module. The types-only loader has a separate publication path. | Both entries obtain the same finalized session IR before any Telora execution; only their later consumers differ. |
| Tool plans | `types/tool-plan.rs::PreparedToolExpression` defers bytecode in a `OnceLock`, but retains lowered syntax and module-local evidence. | Tool bytecode generation consumes the finalized session records and graph IDs. Deferred compilation alone is not the global handoff. |

Paths above are relative to `crates/telora-core/src`. Existing POD inference
nodes, proxy resolution and type arenas are reusable mechanisms. Their current
module-local ownership is not the required architecture.

The types-only data-module path now contributes `{ data: Value }` without reading
JSON/TOML/YAML contents. It records module identity, format and the static export
contract, with no data source or content diagnostics. Syntax, UTF-8 and data
limits remain later loading concerns. The static contract still obtains Value
through the current per-module builtin solver; moving that reference to the
session declaration/type arena remains part of the global migration.

Two further dependencies are important for integration:

- `types/dependency.rs::solve_module_plan` now takes already resolved HIR.
  Module callers obtain constructor roles from the session source graph.
  Ordinary loading and native installation take the prepared HIR by move.
- `heap/type-graph-builder.rs::type_graph_values_in` already consumes graph IDs,
  retains a flat value table, and reserves nominal metadata before following
  recursive bodies. Reuse this conversion mechanism against the finalized
  session arena; it does not require another solver or evaluation of type syntax.

## Two completion boundaries

1. Close resolve for the whole session: inventory source and exports, allocate
   module/symbol identities, link every reference, and require all module
   consumers to use that result. Source declarations determine constructor
   roles; no type solving or VM execution is needed for this boundary.
2. Close types for that same graph: preallocate required slots, apply evidence,
   normalize and diagnose Unknown/Conflicted, then pass one finalized typed IR
   to every tool/runtime compiler. Remove module-owned solutions, descriptor
   bridges, solve/execute interleaving and late inference. Generic instances and
   contextual conversions retain independent slots. Property values remain later
   execution work; record-offset lowering is also later work.

Intermediate branch states need not preserve complete functionality or compile.
There are no additional acceptance milestones for small interface migrations,
and no compatibility fallback should be added to keep such a migration green.

## First integration dependency: resolve before solving

The ordinary analysis entry now also requires an owned HirProgram argument.
It no longer constructs HIR internally from execution roots and interfaces.
Module loading now uses that boundary to consume the same session preparation
as native installation and checking. Native sources enter discovery before
MainWorld creation and are not reparsed during installation. Native catalog
queries also build a real inventory, with no provisional ModuleId fallback.
Missing session import records are errors; loaders no longer reparse missing
syntax or retry name resolution against a live resolver.

Every external HIR reference must carry a source origin, including bootstrap
symbols and Host bindings registered before resolution. Ordinary recovery and
test entry points stop at session resolve diagnostics before creating MainWorld;
native installation also rejects a graph with resolve errors. Such a failed
resolve returns diagnostics and source records, not partially solved type facts.
This does not yet complete global type solving: module-owned inference and
solve/execute interleaving remain the next boundary to replace.

The resolve integration is not yet accepted as fully closed for all consumers.
Two LSP completion regressions currently expect export completion through a
solved dependency result type, even when the requesting source has parse/resolve
errors. `semantic.rs::WorkspaceSnapshot::exports_of` still reads the module's
result type. Replace that consumer with the static export-name inventory; do
not restore early type inference to make these tests pass. The failed resolve
projection currently retains sources and diagnostics but drops its symbol graph.
Preserving that graph for editor queries is the remaining resolve handoff gap.

Validation of this integration: 422 core tests pass, release builds, and the
ontology `check @test/query` command passes in ordinary and types-only modes.
The workspace suite stops at the two LSP export-completion failures described
above; it is not green. No performance comparison was run for this change.

Moving the old HIR constructor into discovery verbatim could not work: its
member-pattern classification depended on solved external interfaces.
Resolving HIR with empty constructor information and fixing names later is not
sufficient either: treating a constructor as a binding changes lexical scopes
and references in the pattern body.

The session declaration index must include declaration kinds and constructor
export identity, with alias/re-export edges. Resolve those facts without type-body
evaluation or full module inference, then build each module's HIR once using the
resulting external-name and constructor facts. A regression fixture must
distinguish an imported constructor pattern from a shadowing local binding,
including alias/re-export cases. Do not equate capitalization with constructor
identity. This is a prerequisite for the reachable HIR inventory, not a reason
to keep dependency-first module type solving.

The types-only entry now has a first integrated implementation of this boundary.
`StaticNames` follows source declarations, type aliases, namespace references and
selected import/re-export names without constructing types. It prepares only
reachable module HIR (including the Value dependency of data modules), then drops
its temporary classification index. The solver takes the prepared HIR by move;
it cannot recreate it from solved interfaces. Bootstrap name resolution uses a
name-only inventory, tested against the bootstrap type contracts.

A regression fixture obtains HIR and distinguishes a re-exported lowercase enum
member from a shadowing local pattern despite a type error in its dependency.
The complete checker still reports that error; after correcting it, checking the
same program succeeds. This is not full session declaration identity or type-slot
ownership yet: imported HIR references and all type solutions remain module-local.

The next integrated step replaces separate HIR-name and type-input import
selection with `ResolvedStaticModule`. Import targets refer to a namespace module
ID or a stable `(ModuleId, export row)` allocated from the source result before
typing. Alias tests check identical target identity before any solver runs.
The types-only solver consumes those targets rather than resolving exported names
again from import syntax. Existing interface selection remains the transitional
type ingress; export-row identity is not yet a canonical source-definition or
inference-slot identity.

Non-prelude `import ... *` now participates in this resolution. Explicit bindings
override open candidates; explicit open providers override the implicit prelude;
duplicate imports from one provider share a target. Multiple providers produce
an ambiguity diagnostic only when a name is referenced outside a local binding.
Constructor-pattern classification participates in that reference check, using
the prepared HIR instead of building HIR again for each ambiguous name.
The enum-constructors fixture, previously failing types-only with `unknown binding
"Make"`, now passes. Paired ordinary/types-only CLI cases cover these rules.

Dependency solving now updates its session interface table in place and returns
only completion, removing whole-interface copies on dependency return and reuse.
Selected type inputs still clone descriptor-based interface fragments. Ordinary
loading still has its separate import preparation; moving it to the shared graph
and replacing selected interface fragments with type-slot edges remain required.

Wildcard imports now retain provider module IDs as search scopes. HIR requests
individual external names through a lookup callback; only actual external
references become wildcard type inputs. Pattern classification probes that turn
out to be local bindings do not create imports. Explicit imports still establish
their declared bindings, and an explicit prelude wildcard participates in normal
ambiguity checking rather than acting only as implicit fallback.

Removing unused inputs exposed two previously implicit dependencies. The HIR for
`@property` now records its required `PropertyAttr` reference. Dependency trait
implementations and property evidence are borrowed separately from selected
export values; their availability does not depend on referencing an arbitrary
export. This remains a descriptor-based bridge until facts and declarations
reside in the same session arena.

Each module's export names are now indexed once before consumer resolution.
The index borrows source names and points to fixed source result rows. Role
solving uses a parallel array with Unknown/Resolving/Known states; queries do
not allocate string-keyed role records or rescan exported fields. Building this
provider-side index does not populate consumer wildcard bindings or classify
unused exports. Tests check that resolution retains the preallocated targets.

After preparing all reachable HIR, export alias resolution now uses the final
local HIR definition IDs to connect duplicate export names to one source row.
Imported re-exports follow their already resolved targets, including namespace
imports. The types-only loader consumes these canonical targets when selecting
type inputs, instead of selecting the same declaration again from each bridge
module. Module dependencies and their trait/property facts are retained.

An independently authored def whose initializer names another def remains a
distinct declaration. This is source binding identity, not equality of inferred
types. Tests cover direct aliases, a re-export chain, a namespace re-export and
that distinction, then run the actual checker against the resolved graph.

The types-only preparation now attaches module-qualified source identities to
HIR import definitions and their references, in arrays indexed by the final HIR
IDs. Declared exports use `(ModuleId, HirDefinitionId)`; synthetic/expression
exports retain a source-row identity and namespace imports identify the module.
Lexical resolution remains explicit, so a shadowing local declaration does not
inherit an import's origin.

Program inference consumes these origins: aliases reuse one imported declaration
slot and scheme within the inference arena; open-import references read that
slot directly. Generic calls still instantiate the shared scheme separately.
Regression tests read both aliases from one initially unknown source slot with
an empty name environment, then verify that resolving the slot resolves both
references. The module fixture calls two aliases of a generic function at Int
and String independently.

HIR now also retains member-access receiver edges. In the types-only resolve
pass, direct and chained namespace accesses follow the provider export index
and canonical export aliases before typing. Their source origins are stored by
expression ID. Missing namespace exports produce resolve diagnostics even when
the provider has an unrelated type error; local shadowing does not inherit the
namespace identity. A nested namespace regression binds both forms to the same
generic source declaration and checks independent instantiations.

Field inference consumes an already registered source binding directly. When a
namespace member's slot has not yet been materialized, the existing descriptor
interface supplies its scheme once to the source binding table. This transitional
ingress still has to disappear with the shared type arena.

The types-only entry now has a session-wide resolution gate. ResolvedStaticGraph
retains the reachable module IDs, including sources with parse errors. Resolve
collects every unresolved HIR reference, along with import/namespace diagnostics.
If any reachable source has a parse/resolve error, the entry creates only syntax
and diagnostic snapshot inputs: it neither constructs TypeStore nor calls a
module solver. Unrelated type errors become diagnosable once resolution succeeds.
A regression verifies two independent unknown names are reported together and
no module exports a type solution across this gate.

This is not yet the complete shared pre-type graph symbol table: ordinary
checking still prepares HIR per module and uses its existing name validation.
HIR type parameters now have their own TypeParameter declaration records and
HirDefinitionIds. Ordinary lexical scopes resolve their references; the temporary
parameter_names set and its External treatment are removed. Signature and body
visits share the identity allocated for the same authored binder location, and
normalization remaps owner parameter IDs together with references. Type-position
classification recognizes parameter definitions as types. A nested same-name
parameter fixture checks distinct binder identities, signature/body reference
closure, scope restoration and zero import lookups for bound parameters.

Audit imports (including wildcard scope records), exports,
local declarations and generated binders against that same criterion; successful
import-origin linkage alone does not prove every symbol class is closed.
Imported slots are still materialized from
descriptor interfaces in separate arenas. The shared session slot owner and
final typed IR remain required; source identity alone does not complete them.

Treat these as separate gates: complete name resolution means every statically
resolvable reference has a source identity (or a resolve diagnostic), including
namespace members, before either checking mode starts type solving. Allocating
the shared type slots is a subsequent requirement, not a prerequisite for calling
the symbol graph complete. Record/trait member choices that actually depend on
types remain explicit constraints for the type phase.

HIR currently normalizes local definition/reference/expression IDs after indexing
each source. Attach cross-module declaration edges after that normalization,
using the final module-qualified HIR identity. Export aliases need edges to that
source record; they must not allocate independent type solutions. The session
then assigns dense syntax-slot ranges without changing declaration identity.

HIR currently marks imported references as `External` plus a name. The session
IR must attach the resolved definition identity at that boundary, not defer the
lookup to runtime binding maps. Module-qualified local HIR IDs can identify
source records while the session assigns dense slot ranges; type unification
does not merge source declarations.

## TypeDesc import boundary

After successful finalization, required skeleton roots can be imported directly
as ordinary VM data. Existing graph materialization provides a useful base:
shared roots reuse `values[TypeId]`; nominal owners are reserved before their
bodies and sealed afterward. The current builder is recursive and its graph IDs
are module-local; neither proves the complete session handoff exists.

The execution consumer owns one mapping for the finalized graph and destination
heap lifetime. TypeDesc construction cannot invoke inference or Telora code, and
cannot compute property values. Generic Bound nodes may describe templates but
do not stand in for concrete instantiated runtime witnesses. Allocation failures
belong to execution handling; they do not reopen solving. The static result
remains usable without creating any VM data.

## Verification and measurement

Use a diamond import fixture to assert shared pending definition identity and
single HIR construction. Exercise cross-module generic calls, shadowing, nested
closures, recursive nominal bodies, contextual conversions, and property
presence without evaluating providers. Check full syntax-slot coverage, not only
exported signatures. Multi-error fixtures must retain independent conflicts and
Unknown diagnostics without a recovery solver.

After dropping inference scratch state, both tool and runtime compilation must
still succeed from the finalized IR. A static entry has no VM/heap capability;
execution consumers have no inference capability. Ordinary and types-only check
must use this same static path. Instrument phase boundaries within one run for
time distribution; subtracting current independent CLI paths is insufficient.

Keep the latest verified local timing/allocation checkpoints in
`static-phase-handoff.md`; this documentation change has no measured speedup.
Measure the ontology query plus small, many-module and highly shared-type cases
after each integrated migration. Do not exchange the full architecture gate for
a favorable microbenchmark or claim cumulative gains across mismatched baselines.
