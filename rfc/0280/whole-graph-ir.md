# Whole-graph typed IR migration audit

Date: 2026-09-10. This is an implementation audit, not a performance result.
The controlling target is RFC 0280's session-wide typed IR.

## Implementation route (supersedes incremental consumer migration)

### Global demand instructions connected to the VM (2026-09-10)

The new codegen no longer uses global_order or rejects syntactic global cycles.
It discovers reachable code, emits an initializer function for each source
global, installs these functions by execution NodeId, then reads the entry using
Demand. Native ABI bindings and injected data are available before initialization.
Ordinary global references in function bodies emit Demand; creating the function
does not evaluate those references. Import aliases retain the defining slot.

The executable carries an immutable task layout into Main. Only the VM Work
world creates and mutates the evaluation state/value arrays. Demand either reads
a saved value or pushes a normal VM call with a completion continuation, so
nested initialization uses the existing explicit VM stack. The same table stays
with the world through eval-with initialization and callback execution. Actual
cycles report the participating global names; failed/unfinished demand sessions
cannot return a successful result. Codegen never owns live evaluation state.

Fourteen codegen tests pass, including recursive functions, syntactic cycles in
uncalled function bodies, unexecuted failing branches, real initialization cycles,
and a counted native initializer proving repeated reads/calls compute once.
Four focused CLI eval tests pass, including recursive demand evaluation, cycle
errors with no partial output, module data and eval-with host inputs.

Property query/provider codegen and solved metadata consumption remain pending;
this checkpoint connects global reads to the VM, not full lazy property support.
Ordinary check, run/serve and complete pipeline replacement remain unfinished.

### Shared demand-evaluation state (2026-09-10)

The agreed execution policy is lazy evaluation of top-level values and property
records in one session. All declarations, code and slots must be ready before
entry dispatch; property values need not all be computed first. Only actual
reads create execution dependencies, including alternating property/global
dependencies. Function construction must not force its body's global references.

The independent execution_graph module builds stable global/property task IDs
from SealedMir. Import/export aliases select the defining task. Each static
property presence record retains one task and its provider reduce order. The
session evaluator uses arrays for pending/running/ready/failed state, values and
the active demand path. Completed values are reused; cycles report their closed
path; failed dependencies can share one diagnostic identity without retry.
Unfinished reduce results cannot be published out of stack order.

Four focused tests pass, including actual sealed-MIR identity/order tests under
reordered inventories. mir-dump --execution-graph exposes the planned task table
without creating a VM. This is an independently tested execution foundation:
the current VM global reads and get_type_prop do not yet use it. Remaining work
is per-task codegen and VM suspension/resumption, solved metadata query linking,
then replacing the eager global-order path. No lazy-property CLI completion or
performance gain is claimed at this checkpoint.

### Module data injection before initialization (2026-09-10)

Codegen emits data relocations using the resolved module identity and solved
Value TypeId. The execution linker reads the selected data sources after static
solving and codegen. VM initialization installs the type image and materializes
JSON/YAML/TOML into Main before running any top-level bytecode. Both eval and
eval-with use this ordering, so even construction of an entry wrapper may depend
on imported data. Host-provided eval-with sources remain separate inputs, checked
against the initialized entry config before invoking its callback.

Data imports and host sources share validation, limits, source tracking and
allocation accounting. No runtime type solving or old Engine path is involved.
Focused CLI tests cover the three formats, repeated import aliases, initialization
depending on imported data, and malformed data accepted by only-types but rejected
by execution without partial output. Ordinary check, properties and run/serve
remain pending; this is assembly progress, not a performance measurement.

### eval-with host input and callback assembly (2026-09-10)

Both eval commands now share the new static preparation path. eval-with checks
the exported entry.Eval identity and the Value result contract in MIR, then
codegen emits its fixed evaluate-call adapter before VM creation. The CLI no
longer prepares PendingModule or invokes Engine for either command.

The new execution entry initializes the wrapper, validates its declared source,
environment and argument config, parses JSON/YAML/TOML inputs with the existing
data validators and limits, and materializes them using the solved Value TypeId.
It calls the precompiled adapter in the same Main/Work world and with the same
quota account as initialization. It neither infers types nor creates metadata
witnesses. Runtime sources join the existing source database so input origins
do not collide with code origins. JSON is published only after success.

CLI coverage passes for all three input formats together, declared environment
filtering, trailing arguments, and Value JSON output. Negative cases verify
missing/mismatched sources, missing env, duplicate config names and rejected
arguments fail before the callback and publish no partial JSON. Ordinary eval
continues to pass its focused contract/output coverage.

Remaining assembly includes ordinary check, module data imports, property
execution and run/serve entry dispatch. Remaining expression lowering (including
match and generic runtime witnesses) still limits which programs these migrated
commands can execute. This milestone reuses the retained VM representation; it
does not implement the deferred runtime-layout RFC or claim performance gains.

### Prioritize pipeline assembly; reuse record storage (2026-09-10)

The agreed priority is replacing the complete compiler pipeline. A distinct
Struct runtime representation, fixed record layouts and related VM redesign
are deferred to a separate RFC. The initial uncommitted representation changes
were withdrawn. Assembly may reuse existing MakeDict/GetField storage and
instructions, while consuming static type-pass conclusions without old resolver
or inference adapters.

The new codegen now supports structural records and nominal Struct initializers
using that retained representation. The type pass records successful record-field
selection; codegen consumes it and emits GetField without revalidating the field
against a type. Native Bool member selection likewise lowers directly to the
existing boolean representation. Property-bearing initializers remain explicitly
unsupported until property execution is connected.

An actual std/entry.main wrapper now compiles and executes through the new path:
construct its config, select evaluate, supply a Context record, call array.length
inside its callback and serialize the resulting Value as JSON 42. This uncovered
and fixed generic type-alias application (actor.Transition): parameter application
must use the declaration's instantiated parameter slots, including unused
parameters, rather than assume the alias body is a nominal constructor.

Validation includes record/config field calls with reordered source fields,
booleans, the real std/entry wrapper and a generic-alias parameter-order/unused-
parameter regression. The 22 type-pass tests pass. Host-side eval-with input
injection and invocation, ordinary check, property/data execution and entry
dispatch remain integration work. No performance claim is made.

### Ordinary eval uses the sealed execution path (2026-09-10)

The `eval MODULE:NAME` CLI now uses inventory discovery, the three MIR passes,
seal, codegen, ABI linking and the new VM execution entry. It validates its
explicit `std/value.Value` output contract through authoritative module exports
and canonical TypeId equality before executing code. A user-defined type with
the same spelling does not satisfy that contract. This command no longer calls
the old Engine; it has no fallback to old compilation or inference.

Dictionary literals lower to MakeDict only when the solved type is Dict.
Record/Struct literals remain explicitly unsupported until their separate
fixed-layout representation is implemented. The new Value JSON consumer uses
the statically selected identity and an iterative traversal of runtime values;
it does not materialize type witnesses or rebuild a second heap data graph.
It detects cycles and retains existing rejection of Bytes and temporal values
for JSON. Output is printed only after execution and serialization both succeed.

A CLI test exercises a nested Value object, native map with a Value-producing
callback, empty arrays/objects, boolean/numeric/string values and escaping. It
also checks rejection of the wrong output type before execution, rejection of
an identically named nominal type, and no partial output for runtime/JSON errors.
The existing plain Value-export eval acceptance test is retained.

This is a vertical CLI migration with remaining language coverage gaps, not
completed assembly. eval-with, ordinary check and entry dispatch still use the
old path. Record layout, property execution, builtin enum lowering, dynamic
generic witnesses and other unsupported operations remain explicit integration
work. No new performance claim is made.

### Solved nominal enum construction (2026-09-10)

The type pass now retains nominal enum member selections as variant indices in
MIR. Codegen consumes those selections and the normalized constructor signature,
without repeating member-name resolution. It emits a `MakeVariant` operation
carrying the original TypeId, variant index and optional payload register.
Payload constructors are ordinary first-class closures and nullary variants are
direct values. Their type declaration syntax is not a runtime dependency.

The VM reads the imported type image for the constructor, checks the bytecode
indices/arity, accounts for allocation, constructs the value and stamps its
solved identity. MIR IDs use a disjoint encoding in the existing value type word;
they are never interned into the legacy runtime TypeStore. `ValueRef` can expose
that original solved ID. Missing type images are invalid bytecode, with no
descriptor reconstruction fallback. Generic constructors requiring dynamic type
witnesses and constructors whose owners have property records are explicitly
rejected by codegen until those execution semantics are connected.

Validation: 9 focused codegen tests, 21 type-pass tests and 9 runtime type-store
tests pass. Coverage includes imported enum aliases, nullary/payload variants,
first-class constructor calls, runtime identity retention, missing-image
rejection and disjoint ID encoding/Unchecked round trips. Runtime metadata,
property execution, builtin enum lowering, matching and semantic JSON consumers
are not yet migrated. No performance claim is made.

### Move the sealed type image into Main (2026-09-10)

`link_entry` consumes the compiled artifact and preserves its sealed type image.
`Vm::execute_linked` moves that image into Main before executing bytecode, using
the retained VM execution loop and quota accounting. Work accesses the same
Main-owned arena. `SolvedExecution` exposes the resulting value, its statically
solved result TypeId and the original image through read-only accessors. The
debug driver's `--run` mode now uses this execution entry.

Eight focused tests pass. The native `array.map`/`fold` test drops the source MIR
before VM creation and checks that execution returns 42, preserves the result
TypeId and retains the exact type/member-definition vector storage addresses.
This verifies ownership transfer without a second copy or descriptor rebuild.

This is the type-image ownership bridge, not completion of runtime type identity
integration. Existing value tags and the retained legacy runtime TypeStore are
not yet unified with MIR TypeIds. Nominal/enum emission, metadata/property
consumers and semantic JSON output still need to consume the imported arena;
ordinary CLI execution has not switched. No performance claim is made.

### Explicit seal and deterministic type image (2026-09-10)

`Mir::seal` is now the publication boundary for codegen. It requires completed
passes, normalized required type slots, no Unknown/Conflicted errors, and proven
bounds. Rejection leaves the original graph intact for diagnostics and query.
`SealedMir` privately holds a read-only borrow: the graph cannot be mutated while
that capability is live. Codegen accepts this capability instead of a raw MIR.
Sealing does not allocate replacement IDs or repeat any solve operation.

The seal extracts a flat `TypeImage` preserving the exact MIR TypeId indices.
Nominal member payloads contain normalized TypeIds, not inference slots. Generic
definitions retain parameter identities, nominal applications retain argument
IDs, and recursive edges refer back to nominal identities. Codegen transfers
this image into the artifact without copying it a second time. The current
borrowed seal performs one table copy so the artifact can outlive the source MIR;
it is not yet a consuming/move-only MIR ownership boundary.

Full-build determinism is a contract: identical inventory, source, roots and
options produce identical IDs and graph content regardless of inventory
enumeration order. This does not promise cross-edit ID stability. The module pass
already sorts canonical names and processes its worklist in ID order. A new
test reverses/rotates inventories and compares the whole MIR dump, sealed type
image, emitted bytecode and native relocations across independent builds.

Eight focused codegen/seal tests pass, including that determinism test, retention
of recursive/generic skeletons after dropping MIR, and preservation of an invalid
graph on seal rejection. VM type-image import, nominal construction and CLI
execution assembly are still outstanding; this milestone makes no performance
claim.

### Native execution ABI linking (2026-09-10)

Codegen now emits native function relocations from resolved declaration IDs and
solved signatures. The execution linker admits callbacks through trusted numeric
module ABI identities, validates arity, and replaces constant placeholders while
sharing the assembled instruction code. Imported aliases do not affect native
identity. Neither codegen nor the linker invokes the old compiler or inference.
Native declarations' signature syntax is excluded from runtime dependencies.

Five focused codegen tests pass, including imported `array.map`/`fold` with
Telora closure callbacks returning 42, unchanged bytecode instructions across
linking, rejected unadmitted native declarations, and rejected ABI arity mismatch.
The `mir-dump --run` driver uses this linker after code generation.

This is not completed CLI assembly. Ordinary `eval` requires a `std/value.Value`
export and JSON output; routing raw primitive results into that command would
change its contract. Solved type skeleton import and nominal/enum construction
must therefore precede that switch. Recursive initialization, type witnesses,
trait dictionaries, property/data execution and remaining expression lowering
also remain open. No new performance claim is made at this milestone.

### First vertical codegen path (2026-09-10)

The new `codegen` module consumes `&Mir` plus an entry SymbolId and emits retained
LIR operations, then uses the LIR assembler to produce bytecode. It does not
import the old compiler/resolver/inference modules or receive a VM. The entry
gate requires completed symbol/type passes, no Unknown/Conflicted slots or
unproven bounds, and no error diagnostics. Each emitted expression must have a
normalized TypeId. Unsupported lowering produces a source diagnostic.

This first path covers primitive literals, tuples/arrays, ordinary acyclic
global dependencies across modules, local bindings, arithmetic/comparisons,
branches, closures/captures, calls, explicit generic application and returns.
Globals are ordered using the already-bound SymbolIds. Captures use those same
identities; no type environment or descriptor tree is rebuilt. The MIR remains
unchanged by code generation. Recursive initialization, native/type linking,
nominal constructors, trait dictionaries, property execution and the remaining
operations are still integration work; ordinary CLI execution has not switched.

`mir-dump --run EXPORT ROOT NAME=PATH ...` is the initial vertical debug driver.
It runs the three static passes, compiles the selected export, then creates the
VM and executes the bytecode. It does not call the old Engine or compiler.
The source and types-only inspection modes continue to avoid creating a VM.

Validation: three focused codegen tests pass, covering captured closures and
branches, imported generic definitions/aliases, unchanged MIR, rejected invalid
and unsupported input, and division by zero failing only in the VM. A real
two-file invocation of `mir-dump --run answer` returned 42 through the new path.
This is the first runnable vertical slice, not completed execution assembly or
a performance result. Further work follows the agreed order: connect the
execution pipeline, then fill rules and complete acceptance coverage.

### Static data module contract (2026-09-10)

Data modules now attach a compiler-owned interface to the same MIR:
`import "std/value" { Value }; decl data: Value; export { data };`.
The module pass does not call the data reader or parse data bytes. Only this
small interface is lowered; no data-file CST is attached. Its explicit import
discovers the normal `std/value` module, and its annotation resolves through that
module's exports. There is no Value-name recognition in type inference and no
second implicit scope rule beyond the prelude import.

The symbol pass treats these declarations/imports/exports as ordinary graph
records. The previous untyped special Data symbol was removed. `check --only-types`
and `query` therefore return a normalized type for the data export before any VM
exists. Parsing and injecting actual data remains an execution-stage operation.

Validation: 27 focused core pass tests and 5 static CLI tests pass. The module
test verifies that the data reader is never called; the type test changes the
inventory's Value export into an ordinary alias and verifies that inference
follows it. A CLI fixture with invalid JSON and `1 / 0` passes types-only checking
and exposes the data export's known type through query, without parsing or
evaluation. Remaining expression rules and execution assembly are still pending.

### Static property and trait evidence graph (2026-09-10)

Property providers and configured factories remain ordinary functions. The
decorator syntax supplies the ordinary call arguments and records the resulting
property type against the target; no provider/function name selects an inference
rule. Field and variant sites supply their structural context types. Providers
are never executed by this pass, including when their bodies unconditionally
fail. Presence records group the same owner/site/property key and retain the
provider sequence for the later metadata stage.

Declaration bounds are lexical assumptions identified by their parameter
SymbolIds. Every generic use instantiates its own bound obligations alongside
its type slots. Trait member access generates an obligation for the receiver's
resolved trait identity and target. Impl bodies are checked against the trait
skeleton using the same record/function constraints as ordinary code.

The pass builds one evidence graph, then propagates proofs to a least fixed
point. Property presence and lexical assumptions supply roots; instantiated impl
requirements supply graph edges. Unproven cycles cannot prove themselves. There
is no speculative evaluation or failure/rollback path. The final MIR retains
evidence nodes, selected impl SymbolIds, substitutions, dependencies, and the
links from source references, so downstream code generation need not select or
solve evidence again. Duplicate bounds, invalid impl targets, overlapping impls,
missing evidence and wrong member signatures are diagnostics. RFC 0260's concrete
impl precedence over property-constrained blankets is preserved by declaration
identity and shape, without special cases for standard trait/function names.

The types-only success gate now also checks every bound outcome and reports
`property_records`, `bound_requirements` and `unproven_bounds` in its summary.
The pass completion marker is set after evidence solving. This completes the
previously explicit missing `Property(P)` proof path, not the whole architecture:
static data contracts, remaining expression rules, metadata capability/value
validation, execution assembly and removal of old execution consumers remain.

Validation: 26 focused core pass tests pass. The added tests cover property
presence with nonexecuting providers, lexical assumptions, missing evidence with
fully known types, trait-to-property dependencies, self-proof cycles, impl
overlap, concrete precedence, field contexts and nominal member signatures.
Four static CLI tests pass, including these facts across module boundaries.
The actual ontology `check --only-types @test/query` now exits 0 with 0 Unknown,
0 Conflicted, 8 property presence records, 4 bound obligations and 0 unproven
bounds. This verifies the exercised rules without evaluating Telora, not complete
language coverage or a performance improvement claim.

### Declaration-driven inference expansion (2026-09-10)

The static scope rule is solely `import "std/prelude" *;`. The symbol pass no
longer injects an intrinsic-name table. Prelude type names are ordinary exports;
their native semantics are linked from trusted module/local slot contracts, not
their spelling. Renaming the primitive declaration at slot 4 preserves its Int
identity; shadowing `Int` with a source alias changes ordinary name resolution.
Unregistered and duplicate native slots produce resolve diagnostics.

Native functions and source functions use the same declared `for(...)` schemes.
Each reference allocates fresh substitution slots; function inputs, callback
parameters/results and the call result constrain those slots. There is no
`array.map`/`find` name dispatch and no copied type environment. Explicit
`f@[T, _]` fills the same instance slots. There is no new implicit let
generalization. `Fn`, tuple/unit syntax and diagnostic macros generate their own
syntax constraints; native type constructors use their linked ABI rules.
Configured decorators constrain both the factory call and its returned provider
signature; the provider's result determines the property result slot. Neither
stage is evaluated.

The expanded pass covers generic calls, nominal/recursive skeletons, constructor
patterns, match/boolean/Never constraints, record construction, indexing and
tuple projection. Type-list constructor arguments remain distinct from ordinary
homogeneous arrays until their context is resolved. Generic instantiation carries
source locations into conflict diagnostics; even a location-less conflict is
reported, and types-only checking cannot return success with retained conflicts.
The MIR dump includes nominal skeletons and declaration generic parameters.

The ontology `@test/query` graph reached zero Unknown and zero Conflicted type
slots with this expansion, without reading data values or executing Telora. This
is a coverage observation, not a performance comparison or a claim of complete
language validation. Generic bound proofs remain unimplemented and are explicitly
diagnosed when encountered: the final ontology run exits 1 with one unsupported
`Property(P)` bound diagnostic in `std/type-property`, despite all slots having
normalized types. Property attachment/trait facts, full decorator
validation, the static data `Value` contract and remaining expression forms still
need work. A solved slot graph alone does not establish those capabilities.

Validation: 20 focused core pass tests and 3 static CLI tests pass. These include
renamed native declarations, ordinary name shadowing, unregistered slots,
independent generic instances, higher-order native signatures, partial explicit
type arguments, diagnostic macro inputs and decorator factory/provider typing.
The CLI test imports `std/array.map` under an unrelated alias, accepts the inferred
`Array(Bool)` result and rejects a `String` result annotation with a located
diagnostic. Query tests retain facts for erroneous programs without evaluation.

The prelude now declares the primitive/constructor native slots needed by the
new pipeline. Execution assembly has not imported these contracts into the old
bootstrap path; ordinary check/evaluation compatibility is not established by
this milestone. No adapter to the old inference or VM was added.

### Early static CLI assembly (2026-09-10)

The agreed next small integration step is now in place: `check --only-types`
and `query` consume the new MIR. `--only-types` is the sole spelling; there is
no `--types-only` alias. This does not declare the third pass complete or start
assembly of execution consumers.

`static_input.rs` builds the workspace/test/embedded-source inventory from
package declarations and source text, then runs module, symbol and type passes
on one MIR. It does not use the old ModuleResolver, Engine, WorkspaceSnapshot,
type interfaces or VM. Embedded sources and native slot contracts are independent
static inputs; no native callbacks or runtime types are constructed. The module
pass accepts a logical-request policy for owner-relative selectors, dependencies
and private/test visibility. Root/import/read failures are MIR diagnostics.

`static_cli.rs` reads MIR diagnostics, source locations, symbol IDs and normalized
type states directly. Query records include their session-local IDs and explicit
Bound/Unresolved/Conflicted or Known/Unknown/Conflicted states. A query can return
facts when the program has errors; an unresolved/unavailable root fails. Module
listing only reads the inventory. Old CLI query snapshot consumers and the old
types-only CLI call were removed, without a fallback. Ordinary check/evaluation
and LSP remain on their existing path pending further assembly.

The bridge is an observation surface for the unfinished type pass, not production
language parity. Valid source can still exercise unsupported rules and fail
types-only checking. Static data modules export a
`data` symbol but its canonical `Value` type is not yet solved. No timing comparison
against ordinary check is meaningful at this point.

Validation: core pass tests and focused CLI tests cover independent known,
unknown and conflicted facts; cross-module and test-module queries; source-position
references; invalid data remaining unparsed and division-by-zero remaining
unevaluated. Complete acceptance and performance coverage remain for full assembly.

### Original independent-pass construction sequence

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
Workspace configuration/catalog construction is now wired through the static CLI input. Data
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

### Third pass construction: independent type arena and evidence kernel

`type-resolve.rs` and `type-resolve/arena.rs` now generate constraints against
the same MIR syntax slots and resolved SymbolIds. Every symbol receives a slot
before evidence is applied; module boundaries do not create separate solutions.
The solver cannot access the old type engine, loader or VM. Resolve outcomes are
read-only inputs, including Unresolved and categorized Conflicted results.

The 8-byte POD slot state distinguishes Unknown, ProxyTo, provisional Structure,
final Known(TypeId), and Conflicted. Provisional constructors reference argument
slots; final constructors reference canonical TypeIds. Equality uses an iterative
queue with proxy compression, structural occurs checks and conflict propagation.
Finalization scans constructors, interns equal resolved structures, and writes
direct terminal states back into the existing slot array. It also records
required Unknown syntax/symbol slots. `types_solved` means the pass has produced
an outcome, not that the program is valid or ready for code generation.

The initial kernel covers primitive values, monomorphic closures/calls, cross-
module binding edges, annotations, tuples, arrays, record fields and basic
branch/numeric constraints. Parser-generated function/tuple/unit type helpers
are lowered to explicit HIR type operations; they are neither user symbols nor
VM calls. Type intrinsics supply occurrence-local rigid evidence so one invalid
annotation cannot contaminate other uses of Int/String.

Five simple type tests and all eleven new-pass tests pass. They check cross-module
calls without changing HIR or symbol outcomes, independent conflicts, Unknown
references alongside known bindings, function/tuple/unit annotations, POD layout
and canonical structure IDs after child equality. This is not the completed
type pass: polymorphic instantiation/generalization, general type constructors,
nominal/recursive skeletons, trait/property facts, the data Value contract and
remaining expression evidence rules still need implementation. Unsupported rules
emit explicit diagnostics; no legacy solver is called. Do not start final
integration or performance claims on the strength of these kernel tests.

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
the same MIR. Ordinary/prelude exports come from the module inventory, not a VM
environment; the former `--intrinsic=NAME` option has been removed.
`mir-dump --types` runs all three new passes and additionally prints provisional
terms, canonical types, symbol type slots, conflicts and remaining Unknown slots.

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
