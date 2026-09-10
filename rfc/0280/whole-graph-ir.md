# Whole-graph typed IR migration audit

Date: 2026-09-10. This is an implementation audit, not a performance result.
The controlling target is RFC 0280's session-wide typed IR.

## Implementation route (supersedes incremental consumer migration)

The agreed development sequence is now:

1. Establish a compiling `telora-core` baseline. Do not require the `telora`
   application to build during the replacement work.
2. Add `module-resolve`, then `symbol-resolve`, then `type-resolve` as independent
   modules. Keep the current implementation available as reference during this
   construction phase; do not continue migrating its individual call sites.
3. New modules must not depend on the implementations they will replace. Reuse
   retained lexical/syntax/source primitives, but do not call the old loader,
   symbol resolver, module interfaces or inference machinery as an adapter,
   bootstrap path or fallback. Required algorithms must live in the new modules
   or in genuinely independent retained primitives.
4. Complete each new module with small direct unit tests, then move to the next.
   Do not require full CLI integration, performance testing or comprehensive
   corner-case coverage at these construction boundaries.
5. Once all three modules are ready, integrate them together and completely
   remove the replaced implementations. All consumers use the new pipeline;
   coexistence during development is not an execution compatibility mode.
6. After integration, run performance evaluation and complete corner-case
   coverage across the full flow.

`module-resolve` owns module inventory IDs, source discovery/parsing and module
edges, including static data-module identity without parsing its contents.
`symbol-resolve` consumes that graph, allocates declaration/export/import and
reference identities, and records Bound/Unresolved/Conflicted outcomes.
`type-resolve` consumes those authoritative bindings, allocates session-wide
type slots, solves evidence and retains normalized Known/Unknown/Conflicted
results. None of these modules receives a VM or executes Telora. Later tools and
runtime consume the finalized type graph and do not reopen resolution/inference.

The existing implementation audit below records reference code and gaps, not
the dependency graph or work breakdown for these new modules. The local
`ProgramTypeOutcome` change belongs to the old implementation baseline: it
collects independent binding conflicts, but its outer module consumer still
exposes only one diagnostic. The new `type-resolve` must not depend on it.

### New pipeline: three passes over one MIR

`mir.rs` owns the evolving session graph. `module-resolve.rs` now allocates the
inventory's dense ModuleIds before reading source, attaches each reachable CST,
and lowers syntax into a separate flat HIR arena (`mir/lower.rs`). This lowering
uses parser AST primitives, never the old HIR resolver. Nodes retain labelled
child edges; reference nodes have explicit resolve slots, and syntax-owned type
slots use the HIR node index. Unannotated parameters and closure results have
slots too. No symbols or types are solved while lowering.

The first pass uses an explicit canonical-name inventory and a text-only reader.
Workspace configuration/catalog construction is not yet wired to it. Data
modules retain their static identity/contract without reading contents. Imports
retain Bound/Unresolved/Conflicted targets; cycles retain graph edges. Each source
is parsed once. Unknown inventory entries remain unloaded until reachable.

`Mir::dump()` reads these same arrays and attached-source identities without
performing resolution, proxy compression or evaluation. The dump shows Pending
references and Unknown type slots before subsequent passes fill them.

Two direct unit tests cover shared dependency loading/data exclusion and
cycles/missing/ambiguous module targets. Do not infer full language or workspace integration from these
module-pass tests; final integration and exhaustive coverage remain later work.

The second pass, `symbol-resolve.rs`, now populates the same MIR with lexical
scopes, declaration/import/export SymbolIds and categorized conflict records.
It indexes all providers before resolving consumers. Export and import aliases
retain their own records while references bind to source declarations. Duplicate
definitions are diagnosed even unused; wildcard candidates become ambiguous
only on an actual reference. Missing/ambiguous module results are consumed as
given, never retried against another loader.

Module namespace fields bind to exported source symbols. Value fields instead
retain an explicit `Member { receiver, name }` type constraint, not Pending name
resolution. Declaration roles identify constructor patterns without evaluating
types. Pattern scopes, sequential lets, closure parameters and generic type
parameters are indexed from the flat HIR. Resolve errors do not interrupt the
pass; its completion invariant is no Pending reference or symbol record.

Four small unit tests cover canonical alias targets with unchanged HIR/type
slot storage, lexical shadowing, unused duplicate declarations vs used wildcard
ambiguity, unresolved references/member constraints, and constructor-pattern
links. The new pass imports only MIR, syntax enums and diagnostics, with no old
resolver or type solver dependency. `type-resolve` is the next independent pass;
full language corner cases are still scheduled after integration.

The `telora-core` example `mir-dump` accepts `ROOT NAME=PATH ...` and prints the
first-pass MIR without constructing an Engine. Example:

```sh
cargo run -p telora-core --example mir-dump -- @src/main @src/main=main.telora @src/shared=shared.telora
```

The example was run with two Telora files and a deliberately absent JSON path:
both CSTs and their HIR/type/reference slots appeared in the dump, and the data
module appeared without any file read. This is an explicit-inventory debug
driver, not the final workspace CLI or configuration integration.

Use `mir-dump --symbols ROOT NAME=PATH ...` to run the symbol pass before dumping
the same MIR. Intrinsic symbol names can be supplied with `--intrinsic=NAME`;
ordinary/prelude exports come from the module inventory, not a VM environment.

## Current gaps

| Boundary | Current evidence | Required replacement |
| --- | --- | --- |
| Module scheduling | `module/type-check.rs::StaticWorkspace::solve` recursively solves dependencies and selects descriptor interfaces using pre-resolved module/export-row targets. | Connect those targets to session definition/type-slot IDs and schedule constraints over shared records. |
| Slot ownership | `types/inference-context.rs::GenericInference::new` takes one HIR program and starts with an empty expression `records` map. `record_type` creates/replaces expression edges while inferring. | Preallocate syntax-owned slots in the session graph, with explicit evidence for contextual conversions. |
| HIR identity | Both checking modes, module loading, generated entry loading and native installation consume HIR from `module/static-names.rs`. Local IDs are qualified by their session ModuleId; bootstrap and Host symbols also have explicit IDs. | Allocate type slots against these source identities without rebuilding HIR. |
| Static handoff | `types/solved-module.rs::SolvedModulePlan` owns one module's arena/evidence; the caller retains HIR until successful analysis publication. `types/type-check.rs::check_module_types` retains only interface and types. | One typed program retains every required syntax type and lowering fact; modules are ranges/namespaces within it. |
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
   module/symbol identities, settle each reference as bound, Unresolved or
   Conflicted, and require all module
   consumers to use that result. Source declarations determine constructor
   roles; no type solving or VM execution is needed for this boundary.
2. Close types for that same graph: preallocate required slots, apply evidence,
   normalize and diagnose Unknown/Conflicted, then pass one finalized typed IR
   to every tool/runtime compiler. Remove module-owned solutions, descriptor
   bridges, solve/execute interleaving and late inference. Generic instances and
   contextual conversions retain independent slots. Property values remain later
   execution work; record-offset lowering is also later work.

During independent construction, preserve compilation and simple unit testing
of `telora-core`. Full application behavior is not a construction gate. During
final integration, temporary incomplete behavior is acceptable; do not add a
compatibility fallback to keep the old flow alive.

## First integration dependency: resolve before solving

The ordinary analysis entry now also requires an owned HirProgram argument.
It no longer constructs HIR internally from execution roots and interfaces.
Module loading now uses that boundary to consume the same session preparation
as native installation and checking. Native sources enter discovery before
MainWorld creation and are not reparsed during installation. Native catalog
queries also build a real inventory, with no provisional ModuleId fallback.
Missing session import records are errors; loaders no longer reparse missing
syntax or retry name resolution against a live resolver.

Every bound external HIR reference must carry a source origin, including
bootstrap symbols and Host bindings registered before resolution. Unresolved
and Conflicted references are explicit results and must be passed into type
solving together with known references. They are not reasons to reject the
resolve artifact. Static diagnostics prevent execution/final output, not the
collection of further independent type facts.

The previous integration's early `diagnostic_inputs` gate is contrary to this
contract and must be removed as part of the handoff. At present ordinary recovery,
tests and types-only still stop there; merely preserving HIR for editor completion
does not implement continued type solving. Do not restore a separate fallback
solver or VM-backed recovery to conceal this gap. Module-owned inference and
solve/execute interleaving remain to be replaced.

The resolve integration is not yet accepted as fully closed for all consumers.
The working tree now carries export names in HIR, preserves its graph in diagnostic
snapshots without rerunning resolution, and exposes optional export types.
The two LSP completion regressions pass without type solving. Standard-library
symbol ownership in ordinary snapshots still needs the same treatment; completion
must not fall back to guessing exports from a namespace variable's type.
Continued type solving after Unresolved/Conflicted results is a separate remaining
handoff requirement, not something these completion tests establish.

Resolve conflicts now have arena IDs and categorized evidence: duplicate
declarations retain separate definition IDs, while used ambiguous wildcard names
retain candidate source IDs. References point at the conflict record. Inference
preallocates Unknown/Conflicted slots for these references and does not reinterpret
them through later same-named environments or schemes. This mechanism is tested
independently of the still-present early session gate; it does not prove that
the driver already continues global type solving after resolve diagnostics.

Execution import preparation now consumes the session's selected imports instead
of rebuilding HIR to decide whether wildcard candidates are referenced. Module
trait/property facts flow along dependency edges even when no export from that
module is selected; fact availability is independent of name selection. Tool
expression preparation no longer builds runtime HIR: solved constructor evidence
drives pattern lowering, and capture collection only checks required runtime
links. Compiler validation accepts HIR resolution outcomes directly, with no
same-named external-binding repair. These changes remove downstream resolution
paths; they do not remove the early diagnostic gate or module-owned type solvers.
Validation: 426 core tests passed before removing the now-unused runtime HIR
constructor; the subsequent CLI build and both open-import tests passed. These
cover unused ambiguity, used ambiguity, explicit/local bindings, repeated imports,
constructor references, and private trait facts from an otherwise unused import
in ordinary and types-only modes. No performance measurement was taken here.

The ordinary workspace no longer invokes a partial solver after analysis failure.
Module solving borrows the authoritative HIR; execution transfers it only when
publishing a successful Analysis. On failure the workspace moves the original
HIR into a diagnostic envelope, retaining source IDs without cloning or resolving
against runtime-derived interfaces. Skipped modules use their unconsumed session
HIR. The runtime-interface partial-analysis adapter and its unavailable-import
bookkeeping have been removed. The direct public partial-analysis API still
exists, but is no longer a module failure/recovery path.

This does not yet retain all inferred facts on an unsuccessful solve: the strict
solver still returns early and discards its scratch graph. The unified total
solver must replace that behavior and publish Known/Unknown/Conflicted facts
from the same solve, including independent facts after a conflict. An empty
diagnostic envelope is explicitly not that completed handoff.
The 426 core tests pass after deleting the recovery path. A targeted ownership
regression also verifies that a static type error preserves the original HIR
definition allocation and leaves the main heap unallocated.
The subsequent three LSP completion tests pass with parse/resolve diagnostics
carried directly into workspace inputs. No timing benchmark was run.

Validation of the conflict/result handoff changes: 426 core tests pass.
The export-completion regressions passed after the static-name handoff. A
workspace run reached language acceptance and failed there (46 other CLI tests
passed); the complete workspace suite is not established as green. No new
performance comparison was run. The prior commit's release/ontology checks
are historical evidence, not verification of these later changes.

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
