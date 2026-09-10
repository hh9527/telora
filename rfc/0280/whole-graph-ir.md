# Whole-graph typed IR migration audit

Date: 2026-09-11. This is an implementation audit, not a performance result.
The controlling target is RFC 0280's session-wide typed IR.

## Implementation route (supersedes incremental consumer migration)

### Detach active runtime host contracts from the old Engine module (2026-09-11)

DataLimits, RunHost and its effect/input/result structures, EvalContext and
EvalSource now live in runtime_host, independently of module loading and type
inference. Root-level exports point directly to that module. The old Engine
imports the shared contracts privately instead of defining or re-exporting
them; EngineConfig and its private empty-host implementation remain with the
old implementation. This removes a dependency that would otherwise prevent
deleting module while retaining the active VM/CLI host boundary.

Definitions, defaults and method signatures are unchanged. This introduces no
new host copying, dynamic inference or compatibility execution path. Validation:
VM tests 48/48 and telora library tests 28/28 pass; final workspace compilation
is checked after removing unused legacy imports. Logs:
/tmp/mir-host-contracts-{vm,cli,check-final}.log.
Old Engine/compiler/types and their descriptor-based runtime paths still need
removal; this is dependency separation, not completion or a performance claim.

### Remove Engine's obsolete asynchronous editor recovery entry points (2026-09-11)

After removal of the old Workspace wrapper, Engine::recover_workspace_async,
recover_workspace_async_in_workspace and their private resolver adapter had no
remaining repository callers. They have now been deleted, including the
WorkspaceBuilder query field/checkpoints and query injection used exclusively
by these entry points. This removes the old editor path that installed native
modules and evaluated code while constructing a recovered semantic snapshot.
The MIR workspace remains the editor implementation; there is no adapter back
to Engine recovery. Synchronous legacy Engine recovery still exists pending
removal of the larger old module/compiler/type implementation.

Validation: cargo check --workspace passes without warnings and telora library
tests pass 28/28, including static editor solving, cancellation, stale revisions,
overlays and LSP requests. No remaining asynchronous Engine recovery references
were found in repository Rust code. Logs:
/tmp/mir-legacy-async-{check,editor}.log. No performance claim is made.

### Remove the obsolete workspace and evaluation-plan shells (2026-09-11)

Deleted the old core Workspace/WorkspaceError API and its Engine-backed
overlay/rebuild wrapper. Repository consumers already use the MIR workspace;
the old implementation had only its own tests remaining. Overlay snapshots,
revision rejection, cancellation and publication are covered by the MIR
workspace tests; a retained-document assertion now explicitly checks that an
edit does not mutate the previous document snapshot. The unused old semantic
snapshot revision setter was removed as well.

Deleted the unused EvaluationPlan/BestEffortSession/FailureArena implementation
and its internal tests. Its only externally consumed type, FailureClass, now
lives in VM error handling; runtime classification and recovery behavior are
unchanged. No compatibility aliases or feature-gated old implementations remain
for these removed APIs. About 1,150 net source/test lines were removed.

Validation: VM tests 48/48, MIR workspace tests 3/3, final telora library tests
28/28 (including all 23 LSP tests), cargo check --workspace clean. The LSP run
exposed an obsolete monomorphic hover expectation: actual MIR query output is
identity: for(A) Fn(A) -> A. The test now uses a valid block-local binding and
asserts its principal signature, consistent with the earlier query migration.
Logs: /tmp/mir-legacy-shells-{vm,workspace,editor-final,check}.log.
The larger old Engine/compiler/type/semantic implementation still exists and
must be removed; this deletion is not a claim that assembly is complete.

### Runtime value-shape requirements move out of codegen (2026-09-11)

Native declarations with non-function signatures are now diagnosed by the
static pass. Record construction also requires a solved Dict/named-field
struct skeleton; Unchecked is inspected through its underlying skeleton.
Both source nodes and materialized generic instance nodes are checked. These
rules append diagnostics after solving and do not discard Known type evidence.
Seal independently enforces the same requirements, including when a consumer
clears diagnostics or tampers with a solved record type.

Codegen's native-signature rejection and record-type-definition scan have been
removed. It now emits the corresponding linkage/construction from sealed input.
This is a publication-contract correction, not a runtime optimization project.
Validation: 68 type-resolve tests, 88 codegen tests and CLI build pass; tests
cover non-function native diagnostics, continued independent inference and
seal rejection after diagnostics are cleared or a record slot is tampered.
Logs: /tmp/mir-value-shapes-{types,codegen,build}.log.
Generic function identity and legacy pipeline removal remain open.

### Source-module admission is checked before symbol/type solving (2026-09-11)

The CLI/LSP shared static inventory now validates source-module declarations
against their retained CST/HIR: explicit exports are required, module-level let
and authored result expressions are rejected, and native declarations require
trusted built-in inventory provenance (a std-prefixed name is insufficient).
This is a workspace admission operation in the new module pass, independent of
the old loader. Low-level embedding clients can still compile expression
snippets and supply host natives. Data interfaces are compiler-owned and are
not checked as authored source modules.

Admission appends diagnostics without aborting or replacing the graph; symbol
and type solving still run. Validation: all 3 module-resolve tests pass,
including five invalid declaration forms, trusted/untrusted native admission,
and continued solving. CLI build passes. The four existing module diagnostic
fixtures now fail check --only-types with the intended static diagnostics,
zero unknown types and zero execution time. compiler-semantics passes 38/38.
Logs: /tmp/mir-module-admission-{tests,build,runtime}.log.
No performance benchmark or full acceptance rerun was performed. Generic
function identity, remaining diagnostic gaps and legacy removal remain open.

### Pattern selection and coverage close in the static pass (2026-09-11)

Constructor patterns and referenced bare variant patterns now retain their
selected boolean/enum/newtype constructor directly on their MIR node. Seal
checks the selected family, index and payload presence against both template
and instantiated node types. Codegen no longer follows alias chains to discover
pattern constructors; it reads the selection and emits ordinary tests/extracts.

The static pass scans HIR in child-before-parent order with a small indexed
coverage fact table. It diagnoses missing finite-enum variants, arms covered by
earlier unguarded arms, refutable ordinary let patterns, and irrefutable let-else
patterns. An unguarded constructor covers its variant when its payload pattern
is irrefutable; guards are never evaluated to prove coverage. Shape validation
also prevents an empty struct pattern from claiming to cover Int or Dict.
Unknown/Conflicted pattern inputs retain their existing diagnostics rather
than triggering another inference attempt. No old pattern/descriptor evaluator
or VM dependency was introduced.

Validation: 66 type-resolve tests, 88 codegen tests and CLI build pass. New cases
cover missing/guarded/repeated variants, catch-all reachability, nested tuple
payloads, newtype lets, aliased booleans, invalid struct shapes and tampered
constructor indices. A fresh full language run reduces failing cases from
230 to 215, including the preceding let-else divergence fix. All 9 query cases
remain green, and test-mode failures remain only module-interfaces and
stdlib-collections. Remaining 213 diagnostic cases include both missing rules
and message/protocol differences; the full migration is not yet accepted.
Logs: /tmp/mir-patterns-{types,codegen-final,build,language}.log.
No language fixtures or runtime instructions were changed, and no performance
assessment was made. Generic function identity and legacy pipeline removal
remain open.

### Let-else divergence is validated without replacing type evidence (2026-09-11)

The old MIR constraint assigned Never directly to a let-else failure branch.
Bottom compatibility then swallowed a real Int/Unit branch, allowing invalid
control flow through seal. The type pass now solves that branch normally and
checks its resulting type for divergence. It reports the violation at the
branch location, retains its actual Known type for queries, and continues
solving independent definitions. Seal independently rejects a non-Never
let-else branch even if diagnostics are removed. Codegen is unchanged.

Validation: 64 type-resolve tests, 88 codegen tests and CLI build pass. Cases
cover Int/Unit/mixed branches, preserved independent evidence, seal rejection,
and valid return/fail!/all-diverging branches. The actual
diag-let-else-never --only-types command now exits 1 with the expected diagnostic
and zero execution time; compiler-semantics passes all 38 language cases,
including return and panic! let-else branches.
Logs: /tmp/mir-let-else-{types,codegen,build,fixed,compiler}.log.

Match coverage/unreachable-arm validation, other diagnostic acceptance gaps,
generic function identity and old-pipeline removal still require work. The
full aggregate was not rerun after this targeted fix; the query audit below
records the preceding aggregate result.

### MIR query acceptance preserves complete type facts (2026-09-11)

Native opaque type display now uses the resolved native declaration and module
identity, e.g. opaque(std/test#Test), instead of Rust's NativeTypeId debug text.
Query acceptance now requires the MIR's preserved generic arguments and native
Bool spelling. The invalid-source query checks authoritative Known Int facts
for successfully parsed later definitions and still requires diagnostics;
it no longer asks for a separate recovery authority. RFC 0256 records these
MIR query contracts. No alternate type graph or descriptor reconstruction was
introduced, and no type arguments were erased to mimic legacy formatting.

Validation: 5 MIR query tests and CLI build pass. A fresh complete language
run passes all 9 query/query-at cases; overall failing cases decrease from
235 to 230. Remaining failures are 228 diagnostic cases and the two generic
function identity cases (module-interfaces, stdlib-collections). This is not
full language acceptance. Logs: /tmp/mir-query-acceptance-{tests,build,language}.log.

Diagnostic audit distinguishes missing rules from message differences. Direct
only-types checks incorrectly accept diag-match-missing-variant and
diag-let-else-never; these are verified static-validation gaps, not formatting
issues. Logs: /tmp/mir-pattern-gap.log and /tmp/mir-let-else-gap.log.
Old pipeline removal and full CLI/LSP acceptance remain incomplete.

### Property applicability is a sealed obligation and a lazy VM check (2026-09-11)

Each concrete ordinary property now carries a PropertyAdmission with the stable
PropertyId of its capability record and its permitted owner-category mask.
The carrier TypeId comes from the admitted native property ABI signature.
Marker aliases retain the native binding identity; a user function named
property is ordinary. Missing carrier declarations and non-marker attempts to
publish the reserved PropertyAttr record produce static diagnostics. Seal
validates the obligation, including rejection of a forged bootstrap exemption.

Capability values remain tool-stage work: codegen emits an ordinary Demand,
field read and mask test before the provider/factory is evaluated. Rejection
uses the existing fail! instruction path at the decorator location. Native
marker records bootstrap without demanding themselves. There is no target
constant evaluation during solving, new VM operation, host value copy or
runtime type inference.

Validation: 63 type-resolve tests, the 87-test codegen suite and 48 VM tests
pass. A subsequent focused run passes both applicability tests, including the
new lazy/cyclic-dependency case (88 codegen tests now exist). The target matrix
covers all six PropertyTarget values against struct, newtype, enum, field and
variant owners, using a computed target and an aliased native marker. Static
tests include a target that would fail if executed, a shadowing user property
function, missing/forged markers and seal tampering. VM coverage confirms that
target rejection and capability failure prevent provider execution and cache
one failure/diagnostic across repeated reads.
Logs: /tmp/mir-property-admission-{types,codegen,vm,final}.log.
CLI build and actual properties (1/1), static-property-evidence (2/2), and
property-target (2/2) language tests also pass; logs use the same prefix with
build/language/evidence/target suffixes. No language fixtures were changed.

Unspecialized generic function identity, diagnostic/query acceptance and old
pipeline removal remain open. No performance assessment was made.

### Generic property applications close before codegen (2026-09-11)

The static instance pass now specializes property templates for every concrete
nominal owner it discovers, including owners introduced by applied member
layouts. PropertyRecord retains a template/concrete distinction and the owner's
GenericInstanceId. Provider expression types, generic references and property
result TypeIds are substituted in MIR using the same instance graph as checks.
Templates remain available as static presence evidence and are excluded from
VM evaluation tasks.

Codegen consumes the closed provider instance and applied member layout. Its
generic-property rejection/type-parameter scan is removed; field/variant context
payloads no longer read the unspecialized source member's type slot. The VM's
existing demand table still owns reduction, values and cached failure state.
Seal checks property/provider types, concrete flags, owner instances, member
indices, duplicate keys and missing applications for materialized owner instances.

Validation: 61 type-resolve tests, 86 codegen tests, 48 VM tests and CLI build
pass. New cases cover Int/String applications, generic property carrier types,
chain reduction, field/variant payload metadata, owners discovered through nested
layouts, and seal rejection of removed instance/record data. The existing VM
object-reuse/failure-cache test now also covers generic owners. Actual language
tests properties (1/1), static-property-evidence (2/2) and property-target (2/2)
pass. Logs: /tmp/mir-generic-properties-{types,codegen,vm,build,language,evidence,target}.log.

Unspecialized function identity, property capability admission, diagnostic/query
acceptance and legacy pipeline removal remain open. No performance assessment
or language fixture changes were made.

### Principal generic contracts retained in MIR (2026-09-11)

MIR now retains a session arena of alpha-normalized quantified type schemes.
Binder-dependent skeletons use scheme node IDs; closed subtrees reuse existing
TypeIds. Contracts include declared generic bounds and their parameter indices,
so constrained and unconstrained functions cannot acquire the same scheme.
Alpha-equivalent contracts share an ID while their function symbols remain
distinct. Construction uses stable symbol order and iterative graph traversal.
MIR dumps expose the arena, and seal checks signatures, binders, bounds and edges
against the solved source information.

This is representation groundwork only: first-class unspecialized references
do not yet consume these schemes. The module-interfaces and stdlib-collections
function identity failures remain open. No new codegen or VM behavior, runtime
inference, arbitrary default types or performance claim is included.

Validation: 60 type-resolve tests pass, including alpha-equivalence, independent
TypeId reuse, deterministic reconstruction, distinct bounds and seal rejection
of damaged binders or missing bounds. All 85 codegen tests also pass.
Logs: /tmp/mir-schemes-bounds.log and /tmp/mir-schemes-bounds-codegen.log.

### Interpreter plans generate ordinary executable adapters (2026-09-11)

Codegen now consumes InterpreterPlan to emit an outer witness factory and an
inner adapter closure. The plan selects direct forwarding or the existing
builtin Dyn pack operation at each parameter position. Packing retains the
witness and original value handle. No opcode, hidden binding, type inference
or runtime type reconstruction was added. Operand evaluation stays inside the
invoked adapter, after factory creation, with ordinary lexical capture behavior.

Factories use the VM's existing memoized-interpreter table. In solved sessions
its keys now consume SolvedType IDs directly instead of parsing legacy Dict
metadata. The legacy metadata branch remains for the not-yet-removed old
pipeline; solved sessions cannot fall back to it. Local generic interpreter
definitions use the existing non-expansive closure allocation/sealing path.

Validation: 85 codegen tests, 48 VM tests and CLI build pass. New runtime cases
cover reordered/aliased witnesses, mixed/repeated arguments, same/different
factory identity, lazy failing operands, enclosing local captures and
metadata-only factories. The actual interpreter verified_case and its language
aggregate checker pass. Remaining test-mode aggregate failures are
module-interfaces and stdlib-collections (unspecialized function identity);
diagnostic/query gates and old-pipeline removal remain incomplete.
Logs: /tmp/mir-interpreter-runtime-{codegen,vm,build,language}.log.
No performance assessment or fixture changes.

### Interpreter adapters are solved and sealed in MIR (2026-09-11)

MIR lowering now retains only the authored interpreter operand, discarding the
legacy parser-generated adapter. Hidden pack names and synthetic closures no
longer enter the graph's symbol/type obligations. The new interpreter rule
consumes the resolved explicit generic signature: it maps each quantified
parameter to a unique TypeOf witness, classifies each inner parameter as a
direct pack or independent pass-through, rejects nested interpreted parameters
and dependent results, and constrains the operand to the derived erased ABI.
Aliases such as Witness(T) = TypeOf(T) work without spelling-based checks.

The session stores an InterpreterPlan beside HIR, and MIR dumps expose it.
Seal validates the plan against the closed outer/inner/operand types, including
witness indices and pack relationships. A tampered plan cannot seal. The rule
waits for incomplete evidence without allocating new type terms on every retry.
No VM access or dependency on the replaced type/compiler modules was added.

Validation: 58 type-resolve tests, 84 existing codegen tests and CLI build pass.
Tests cover reordered/aliased witnesses, mixed and repeated arguments,
metadata-only factories, Never-returning operands, invalid contracts and seal
rejection of a changed plan. Actual check --only-types @test/interpreter/testee
now reports status ok, zero Unknown/Conflicted and zero execution seconds.

This is the static adapter milestone, not interpreter execution completion:
the actual test command now reaches the explicit missing Interpreter codegen
diagnostic. Mechanical adapter generation and its runtime identity/cache
behavior remain to be connected. Other migration gaps and legacy removal
remain. Logs: /tmp/mir-interpreter-{types,codegen,build,check,runtime}.log.
No performance assessment or fixture edits.

### Initialization acceptance follows lazy session semantics (2026-09-11)

The old initialization fixture assumed that an unread top-level failure was
eagerly executed before test thunks. That contradicts the adopted session-wide
lazy demand strategy. Runtime behavior was already correct; this milestone
changes acceptance rather than restoring eager module initialization.

The fixture now distinguishes a first demanded failure caught by should_fail_with,
a repeated cached failure caught by should_fail, an unread failing initializer,
and a Test export whose own initialization fails. The old substring expectation
is replaced by a structured checker requiring two passed thunks, one failed
export, exactly one initialization diagnostic, no abort and a nonzero exit.

Validation: the complete language aggregate reports test/initialization=true.
The underlying negative test session intentionally reports 2 passed / 1 failed.
No Rust implementation changed and no core rebuild or performance test was
required. Log: /tmp/mir-initialization-language.log. Remaining test-mode failures
are interpreter, module-interfaces and stdlib-collections; the last two involve
unspecialized generic function identity comparisons. Other diagnostic/query
gates and legacy removal remain outstanding.

### Decoded dictionaries retain their solved identity (2026-09-11)

The dictionary decode completion task dropped its known target TypeId. A
record containing Dict(String) therefore decoded to the right contents but
failed equality with the authored record. The task now carries that solved
ID and attaches it to the newly decoded dictionary handle, matching ordinary
dictionary construction. No runtime type reconstruction or data copying was
added.

Array, Tuple and native Option values retain their existing runtime
representation: ordinary construction does not stamp identity tags on those
values. Their static types remain in MIR/TypeImage. Regression coverage checks
the dictionary tag and these structural containers together, including present
and absent Option values, and asserts full round-trip record equality.

Validation: 84 codegen tests, 48 VM tests and CLI build pass. Actual codec-schema
now passes its complete verified_case (1/1), including recursive structures,
schema output, parser equivalence and JSON formatting. Logs:
/tmp/mir-dict-{codegen,vm,build,language}.log. No perf run or fixture changes.
Remaining test-mode failures from the preceding aggregate are initialization,
interpreter, module-interfaces and stdlib-collections. Other diagnostic/query
gates and legacy removal remain incomplete.

### Empty Option instances close from constructor evidence (2026-09-11)

An unconstrained use such as option.is_some(None) left the Option argument and
both generic references unknown. At the fixed point, the solver now follows
resolved declaration/alias identities to the already selected empty Option
variant. An unknown argument at that concrete use receives Never: the empty
constructor has no payload evidence. This does not match the spelling None.

This happens after ordinary constraints and generalization: explicit Option(Int)
contexts retain Int, and an exported generic empty declaration retains its
parameter. Arbitrary generic functions returning Option(_) receive no such
evidence and remain unknown when genuinely unconstrained. Codegen is unchanged.

Validation: 83 codegen tests, 56 type-resolve tests and CLI build pass. Tests
cover renamed imports/aliases, direct member syntax, contextual Int, retained
generic declarations and rejection of an unknown generic native result.
Actual stdlib-semantics now passes 5/5 and its aggregate checker passes.
Remaining test-mode aggregate failures: codec-schema, initialization,
interpreter, module-interfaces and stdlib-collections. Other diagnostic/query
gates and legacy removal remain outstanding. Logs:
/tmp/mir-empty-{codegen,types,build,language,aggregate}.log. No perf run or fixture changes.

### Match failures keep the established runtime category (2026-09-11)

Codegen now emits the existing Fail instruction when no match arm accepts a
value, preserving NoPatternMatched instead of generating Panic. The message
again identifies that no arm accepted the value. Solved Dyn field reflection
also includes the requested index in its out-of-range diagnostic.

Validation: 82 codegen tests, 48 VM tests, CLI build and the actual
runtime-failures suite (11/11) pass. Logs: /tmp/mir-fail-{codegen,vm,build,language}.log.
No fixture change or performance run. Remaining migration gaps in the previous
audit still apply, except runtime-failures is now passing.

### Decode diagnostics retain alternative failures and subjects (2026-09-11)

The solved untagged decoder discarded every rejected candidate's reason and
blame subjects. Its final no-match error therefore lost both the useful nested
failure explanations and the original failing field location. Candidate trials
now retain VM blame handles. A no-match result includes their messages and
retains the first concrete failure's subjects; ambiguity retains the input as
its subject. Successful alternatives still discard rejected candidates, and
VM failures remain outside the alternative data-mismatch mechanism.

The solved text parser also restores format-specific virtual source names
(<json string>, <yaml string>, <toml string>), and missing-field/untagged
messages retain the established diagnostic contract.

Validation: 82 codegen tests, 48 VM tests and CLI build pass. The language
aggregate now passes codec-construction-check (11/11), parse-construction-check
(9/9), decode-errors (8/8), and the decode-provenance checker. The latter checks
expected failing cases and their source positions, not successful decoding.
Its untagged diagnostic retains input.json line 6 column 11 and reports both
$.count: expected Int and $.count: expected Float.

Remaining test-mode aggregate failures: codec-schema, initialization,
interpreter, module-interfaces, runtime-failures, stdlib-collections and
stdlib-semantics; other diagnostic/query gates and legacy removal also remain.
Logs: /tmp/mir-decode-diagnostics-{codegen,vm,build,language}.log. No perf run
or fixture changes. This does not claim full migration completion.

### Solved decoding accepts temporal data variants (2026-09-11)

TOML parsing already produced correctly typed Value.LocalDate/LocalTime/
LocalDateTime/OffsetDateTime values. The solved decoder only admitted enum
input encoded as String or a single-field Object, rejecting those temporal
values before consulting their matching declared payload type.

Temporal data now selects the declared enum member by its external name and
decodes the text through that member's solved payload TypeId. It uses the
ordinary variant construction/check tasks, including nested payload checks.
The text handle and provenance are retained; only a quota-accounted Value.String
wrapper is introduced inside the VM. No type reconstruction or host-world
copy is involved.

Validation: 82 codegen tests, 48 VM tests and CLI build pass. Regression tests
cover all four temporal variants, mismatching payload types, missing members
and a checked newtype payload rejection. Actual data-modules now passes 3/3
(JSON, YAML and TOML). Logs: /tmp/mir-temporal-{codegen,vm,build,language}.log.
No performance run or fixture change.

Further investigation confirms two separate remaining gaps: collections
compares unspecialized identity functions without concrete argument evidence;
interpreter expressions have neither a MIR evidence rule nor closed hidden
pack references. The latter still carries parser-generated elaboration into
MIR and needs a statically derived adapter plan. Neither gap is fixed by this
milestone. Other acceptance failures and legacy removal remain outstanding.

### Generic argument completion preserves directional evidence (2026-09-11)

Imported generic signatures exposed an ordering bug: Fit equated an instance
slot with Unchecked before the source supplied its nominal constructor. The
solver now retains the pending instance producer and waits for its skeleton
before consuming the directional boundary. Retiring a producer advances the
solver revision even if its parameter already aliases the target.

An Unchecked argument also waits for independent parameter evidence before
choosing an unconstrained template slot. This allows the TypeOf argument of
dyn.pack to establish the checked owner. At the fixed point, a genuinely
unconstrained identity parameter keeps Unchecked. Conversion targets remain
explicit MIR value adjustments; codegen and VM were not changed.

Validation: final 80 codegen tests, 55 type-resolve tests and CLI build pass.
Regression coverage includes imported generic acceptance/rejection, identity
preserving candidate identity and a TypeOf-driven checked argument. Actual
construction-boundaries passes 16/16, checked-recursive-types passes 5/5.
The language aggregate still fails codec/diagnostic/data/interpreter and
other cases; full migration and legacy removal remain incomplete. Logs:
/tmp/mir-fit-{codegen,types,build,language,recursive,aggregate}.log.
The aggregate preceded the final producer-retirement revision bookkeeping;
focused suites and both actual cases were rechecked afterward. No perf run.

### Local struct updates inherit their solved source owner (2026-09-11)

An unannotated local binding of a struct update incorrectly marked the update
as a materialized anonymous record. Because updates share the source type,
that marker propagated into the source and prevented its explicit nominal
annotation from completing record construction. Construction-origin analysis
now reserves this marker for record construction and projection: updates
inherit their source owner. No codegen inference or VM fallback was added.

Validation: 79 codegen tests, 55 type-resolve tests and CLI build pass. The
local imported checked-struct regression executes to the expected value.
Actual construction-check-once passes 2/2: copies emits one check warning,
and merge_chain emits three (initial construction plus two updates).
Logs: /tmp/mir-update-{codegen,types,build,language}.log. No performance
measurement was made; remaining migration gaps from the prior audit remain.

### JSON Schema consumes solved IDs and VM property results (2026-09-11)

The solved VM now implements json.schema_with instead of reporting an
unimplemented InvalidBytecode operation. An explicit work stack traverses the
closed TypeImage. Nominal TypeIds key recursive $defs/$ref links; applied
layouts supply fields, enum payloads and newtype bodies. Output is constructed
directly as typed Value handles in the VM. The solved path does not decode
runtime metadata into CodecType, build a CodecNode schema tree, reconstruct
types, or transfer property values through a host world.

Schema generation demands codec marker/rename properties through the VM's
existing evaluation table, suspending and resuming with a native continuation.
It uses the same failure cache and does not retry or duplicate diagnostics.
Codegen admits schema_with as a property consumer so the compiled artifact
installs provider thunks. Generating a schema does not run construction checks.
Unsupported mappings and invalid rename/untagged rules report recoverable
TypeMismatch errors. Native enum payloads use their solved argument mapping.

Real schema equality tests also exposed missing Dict(Value) payload witnesses
in parsed Value.Object values. Parsing and codec/schema construction now share
a TypeImage lookup for that existing applied payload TypeId. The parser stamps
the original dictionary handle and preserves its provenance; it neither copies
the parsed dictionary nor weakens equality. This applies to solved data-plan
materialization as well as JSON/YAML/TOML parsing.

Validation: final 78 codegen and 48 VM tests pass; CLI build passes. Exact schema
output and equality with parsed output cover primitives, tuples, recursive
structs, optional fields, newtypes, Result payload order, renamed fields,
untagged enums and text codec markers. Additional coverage checks recoverable
mapping errors, no construction-check execution, and one cached failure for
repeated nested schema property demands.

The actual language aggregate now passes enum-codec, newtype-metadata (7/7)
and result-nominal. No fixture was changed. The aggregate still fails other
codec/inference/runtime and diagnostic/query cases: codec-schema now reaches
its assertions but still fails one; this milestone does not claim complete
codec acceptance. Legacy module removal remains incomplete. Logs:
/tmp/mir-schema-{codegen,vm,build}.log and
/tmp/mir-schema-final-language.log. No performance measurement was made.

### Newtype facets and constructor patterns close in MIR (2026-09-11)

Newtype references previously shared the declaration's Meta slot even in value
positions. First-class constructor annotations therefore conflicted with the
declaration, while codegen inspected declaration syntax to guess when a Meta
expression should generate a constructor. The type pass now retains the
declaration's skeleton and solves each value reference separately. Explicit
type positions and type-application evidence select the type facet; otherwise
a newtype reference obtains Fn(payload) -> Owner and a NewtypeConstructor
selection. Generic type applications used as constructors use the same rule.
The old Meta-call value-construction branch has been removed.

Constructor patterns retain their newtype selection in MIR as well. Ordinary
functions and value aliases remain invalid newtype patterns, preserving the
existing declaration-resolved pattern contract. Type aliases and imported
declarations retain their identities. Sealing checks constructor signatures
against the owner's applied payload layout, including generic instance types.
Codegen reads the selected signature or pattern operation; it no longer
inspects a declaration initializer to discover a newtype constructor.

Validation: 76 codegen, 55 type-resolve and 48 VM tests pass; CLI build passes.
Tests cover first-class and inferred-polymorphic constructors, explicit generic
aliases, type aliases, nested payloads, static rejection of function/value-alias
patterns, and sealing rejection for a mismatched constructor payload signature.
The actual newtype-constructors fixture passes 11/11 and newtype-tool-stage
passes 2/2, including a final targeted run after pattern-contract validation.
The newtype-metadata fixture executes 6 passing cases; its remaining schema
case reports the existing unimplemented solved-witness schema generator.
The negative value-alias pattern fixture now reports a static conflict with
zero Unknown slots under check --only-types. No language fixture was changed.

The full language aggregate still fails other codec/schema, diagnostic/query
and runtime cases. Legacy module removal and complete acceptance remain
unfinished. Logs: /tmp/mir-newtype-facets-{codegen,types,vm,build,language}.log,
/tmp/mir-newtype-facets-final-{constructors,tools}.log and
/tmp/mir-newtype-facets-pattern-alias.log. No performance measurement was made.

### Dyn projection is an ordinary generic function (2026-09-11)

The parser previously rewrote any `namespace.project@[T](value)` by spelling
into DynProject, even when the binding belonged to a user module. The new
type pass correctly had no evidence rule for this obsolete special node.
`std/dyn.project` is now an ordinary exported generic function implemented as
`project_with(T.type, value)`. Generic instance solving already closes that
metadata reference, so ordinary codegen emits the witness without a new
inference rule, optimization pass, VM operation or runtime type reconstruction.

Removed the DynProject AST/MIR variants, parser rewrite, old analysis and
compiler branches, and the obsolete imported-Dyn-namespace name tables. The
remaining legacy modules only received the matching variant/argument removal;
they are not fallback consumers for this feature. RFC 0252 records that its
old dedicated-sugar strategy is superseded by closed generic instances.

Validation: 75 codegen, 54 type-resolve and 8 parser tests pass, and CLI build
passes. Coverage includes explicit projection, renamed imports, transitive
generic calls, contextual function values, unequal nominal identity, and a
user module's unrelated generic function named project. The actual reflection
fixture now passes (1/1), without fixture edits. The full language aggregate
still fails remaining newtype, codec, diagnostic/query and other cases;
old-path removal and full acceptance remain incomplete. Logs:
/tmp/mir-dyn-project-{codegen,types,parser,build,language}.log.
No performance measurement was made.

### Native property targets and canonical member indices (2026-09-11)

PropertyTarget now uses its native enum identity and ordinary MIR enum member
selections for all six categories. Aliases, computed targets and constructor
patterns use the normal pipeline; the special integer-valued member selection
has been removed. The native property adapter maps enum values to capability
bits, reduces prior bits and stamps its result with the already solved
PropertyAttr return TypeId. It does not reconstruct a type at runtime.

Nominal skeleton members are sorted by name during static definition setup,
as required by RFC 0259. Member syntax and payload slots stay attached to their
members, so property providers, reflection, Dyn access and constructor selection
consume the same indices. Native enum indices follow the same ordering;
Result/FoldControl payload argument positions and PropertyTarget capability
bits are explicitly independent of those indices. Codegen remains mechanical.

Three language fixtures renamed their explicit `property` namespace import to
`props`: the original alias shadowed the implicit prelude function used by
`@property`. Their assertions were not relaxed. Focused compiler tests were
corrected where they incorrectly expected source-order member indices.

Validation: 54 type-resolve, 74 codegen and 48 VM tests pass; CLI build passes.
The actual language aggregate now passes properties (1/1) and property-target
(2/2). The full aggregate still fails other newtype, codec, reflection and
diagnostic/query cases. Applying a property carrier still needs tool-stage
capability validation; this milestone does not claim that enforcement or
complete enum codec coverage. Old-path removal and complete acceptance remain
unfinished. Logs: /tmp/mir-canonical-{types,codegen,vm,build,language}.log.
No performance measurement was made.

### Construction origins prevent retroactive nominal branding (2026-09-11)

Record shape compatibility previously proxied the actual record slot to the
expected nominal slot. Later use could therefore change the original
initializer's TypeId, and mechanical codegen would stamp an existing anonymous
value with an owner it never constructed. The static solver now retains
source-level existing-record evidence in a dense scratch table. Unannotated
binding constructions keep their ownership; record shape checks do not merge
existing and fresh construction roots. Equal finalized shapes still share a
canonical TypeId. Structural Dict compatibility remains distinct from nominal
ownership.

Comparison constraints consume settled declaration/call evidence before
literal defaulting. Existing operands preserve their record origins; direct
aggregate literals, projections and resolved constructors retain contextual
construction. Constructor aliases are followed through SymbolId and solved
member selections, not their spelling. Ordinary generic function results do
not acquire constructor status. All scratch evidence is discarded with the
solver; codegen and VM receive the final TypeIds without new inference,
environment copying or runtime compensating branches.

Validation: 53 type-resolve tests and 73 codegen tests passed; CLI build passed.
Static coverage checks distinct anonymous/nominal identities and deterministic
MIR dumps. Execution coverage checks stored records, spread references, shared
and independent generic parameters, branches and direct literal context. The
final actual language aggregate passes nominal-equality (22/22),
enum-constructor-context, struct-projection (including provenance) and
tuple-types without fixture edits. Metadata array and constructor-payload
contexts remain covered by those language fixtures. The full aggregate still
fails other codec/newtype/property and diagnostic/query fixtures; old-path
removal and complete acceptance remain unfinished. Logs:
/tmp/mir-record-origin-final-types.log,
/tmp/mir-record-origin-final-codegen.log,
/tmp/mir-record-origin-final-build.log and
/tmp/mir-record-origin-final-language.log. No performance measurement was made.

### Native codec output retains its solved dictionary witness (2026-09-11)

The remaining enum-constructor encoding comparisons exposed a missing type
stamp on native-generated Value.Object payloads. The outer Value owner and
encoded children were correct, but the dictionary lacked the Dict(Value)
witness carried by the equivalent source expression. The encoder now reads
that payload TypeId from the target's already applied layout, checks its
contract, and stamps the existing dictionary handle. It does not infer or
allocate a type, copy a container, or relax runtime equality's identity checks.

Encoding Function and Type/TypeOf values now reports the established language
errors (Function has no JSON codec / cannot encode Type) as TypeMismatch. They
were incorrectly reported as unimplemented InvalidBytecode operations, which
could abort testing before subsequent expected-failure cases ran. Unsupported
source values are distinct from a malformed closed bytecode/type image.

Validation: final 72 codegen tests passed, with tests comparing exact payload
witnesses and encoded records, dictionaries, nested objects, arrays and enum
variants against source constructors. The 45 vm::tests tests passed after the
witness fix; final CLI build passed. The actual language aggregate after the
witness fix passes enum-constructors (11/11) and enum-binding-origins. After the
error classification refinement, the targeted actual encode fixture passes
5/5 with no abort. The complete aggregate still has other failures and was not
repeated for the final two error arms. Nominal-equality's existing-value cases,
other semantic gaps and old-path removal remain unfinished. Logs:
/tmp/mir-codec-witness-final-codegen.log, /tmp/mir-codec-witness-vm.log,
/tmp/mir-codec-witness-final-build.log, /tmp/mir-codec-witness-language.log and
/tmp/mir-codec-witness-final-encode.log. No performance measurement was made.

### Metadata value equality is distinct from type equality (2026-09-11)

Binary equality formerly unified its operand slots immediately. As a result,
Int.type != String.type demanded Int == String, and comparing an unresolved
metadata match result to Int.type could force the match to TypeOf(Int) before
its broad Type branch was solved. Equality now has a separate static worklist
constraint. Known Type/TypeOf values are comparable without equating their
represented types. Other values retain ordinary type compatibility checks.
At a fixed point, unresolved comparison operands receive evidence: metadata
comparison supplies broad Type, while ordinary comparison shares operand type
evidence. This is constraint solving before generalization, not execution or
failure recovery. Codegen and VM comparison instructions are unchanged.

Validation: 52 type-resolve tests and 70 codegen tests passed; CLI build passed.
Tests retain exact TypeOf for identical metadata branches, broad Type for mixed
metadata branches, infer a Type-taking metadata predicate, and reject ordinary
type mismatches and invalid TypeOf annotations. Runtime tests compare actual
distinct/equal TypeIds and exercise the enum payload/match path. Actual
test/prelude-constructors now passes. test/enum-constructors now executes and
passes 9/11 cases; ownership/imported still fail comparisons of encoded values.
The complete language aggregate remains failing. A wider metadata-name test
filter also exercised old module tests: four fail, three explicitly at the
known legacy native property registration boundary. Old-path removal and full
acceptance are still outstanding. Logs:
/tmp/mir-metadata-equality-all-types.log,
/tmp/mir-metadata-equality-all-codegen.log,
/tmp/mir-metadata-equality-build.log,
/tmp/mir-metadata-equality-language.log and
/tmp/mir-metadata-equality-tests.log. No performance measurement was made.

### Constructor and function aliases retain generic instances (2026-09-11)

Member imports lower to ordinary alias bindings. Previously only closure
literals were eligible for implicit generalization, so imported constructors
shared one set of argument holes: one use could constrain every other use, and
explicit applications were rejected as non-generic. Non-expansive function and
enum-value aliases now receive independent reference instances, including
nullary variants. Alias eligibility checks the receiver/callee chain; a field
selected from a call result is not silently re-executed as a generic initializer.
Captured unknowns, recursive groups and outstanding operand constraints retain
the existing monomorphic restrictions.

Aliases preserve the source scheme's parameter order. Selected variants use
their owner's order, including Err's Result(T, E) rather than payload-first
(E, T). Direct member applications constrain the receiver's existing instance
slots; native enum families use the positional slots established by native
constructor rules, not source spelling. All decisions occur during static
solving. Local codegen consumes finalized instance signatures and emits the
existing function sealing or register move operation; no runtime type solving
or container copying was added.

Validation: 51 type-resolve tests and 69 codegen tests passed; the CLI build
passed. Tests cover cross-module re-exports, local aliases/captures, native
family renaming, nullary/payload variants, parameter order, incorrect arity and
conflicting explicit arguments. The prior test asserting that a generic
function alias must be monomorphic was moved to positive coverage. Actual
language fixtures were unchanged. The full language suite still fails: the
enum-constructors fixture's generic-application and Int/String cross-use
diagnostics are gone, but its metadata comparison still incorrectly constrains
a broad match result to TypeOf. That remains a separate static equality/join
gap, alongside the other semantic gaps and old-path removal. Logs:
/tmp/mir-member-instance-final-types.log,
/tmp/mir-member-instance-final-codegen.log,
/tmp/mir-member-instance-final-build.log and
/tmp/mir-member-instance-final-language.log. No performance measurement was made.

### Demand publication preserves value origin (2026-09-11)

The first lazy export/property read formerly rebased a generated initializer
origin through its Register return target, while cache hits returned the cached
value directly. Publication now preserves the initializer's existing origin
and returns the same value without read-site rebasing. This changes provenance
flags only: the heap handle and solved type stamp are retained, with no payload
copy or recursive traversal. Failure continuations are unchanged.

Validation: 67 codegen tests and 48 VM tests passed. The final focused test also
checks that a nominal record retains its solved type, raw handle and location
across first/cached reads and an identity-function return. The actual language
fixtures check-result-provenance, struct-projection-provenance,
struct-update-provenance and tuple-spread-provenance now pass without changing
their expectations. The full language aggregate still fails other fixtures;
this is not a complete provenance audit or full migration acceptance. Logs:
/tmp/mir-demand-origin-codegen.log, /tmp/mir-demand-origin-vm.log,
/tmp/mir-demand-origin-final-tests.log and /tmp/mir-demand-origin-language.log.
No performance measurement was made.

Codegen remains a mechanical consumer of sealed MIR. Types, inferred generic
arguments, call targets and value adjustments belong to the static result;
missing evidence must be fixed there rather than inferred during emission.
Additional optimization passes remain outside the current integration scope.

### Mechanical tail calls preserve native completion boundaries (2026-09-11)

Codegen now propagates syntactic tail position through function/block results,
if branches, match arms and explicit returns, emitting the existing TailCall
operation. Callee evaluation, arguments, conditions and intermediate operands
remain ordinary calls. MIR value adjustments and return-boundary construction
checks disable tail transfer when work remains after the call. This is the
existing language tail-call contract, not a new optimization pass.

The VM retains a frame carrying a native continuation until a tail-transferred
callee completes, then performs an internal return. Native continuations own
diagnostic recovery and lazy-task success/failure publication; dropping their
frame before synchronous native dispatch could lose those effects. Subsequent
tail recursion has a Register return target and replaces frames normally, so
retention is bounded by pending native work, not recursion count. These retained
completion boundaries do not count as additional bytecode call depth; their
native continuation depth still counts. No payload is copied across worlds.

Solved checked casts now distinguish primitive representation errors from
nominal identity errors and retain nested field paths. Their validation and
construction behavior still consumes the sealed TypeId/layout image. The
existing cast test's generic error expectation was updated to the restored
specific diagnostic contract.

Validation: 66 codegen tests passed, including codec construction-check
failures caught by with_diagnostics. After the final logical-depth refinement
and diagnostic-scope recursion assertions, all 48 VM tests and 6 tail-filtered
tests pass. Tests cover 2000-step direct/mutual/generic recursion, match/return
tails, retained non-tail arithmetic, return construction checks, and successful
and failing long recursion inside a diagnostic scope. The actual
test/compiler-semantics fixture now passes all 38 cases. Its last language
aggregate preceded only the logical-depth accounting refinement; the complete
language suite still fails other fixtures. Provenance/remaining semantic
contracts and old-path removal remain unfinished. Logs:
/tmp/mir-cast-tail-codegen.log, /tmp/mir-cast-tail-final-vm.log,
/tmp/mir-tail-final-tests.log and /tmp/mir-cast-tail-final-language.log.
No performance measurement was made.

### Callable value evidence, metadata joins and array bottom evidence (2026-09-11)

Function parameter/result slots now carry provisional value-domain evidence in
a dense solver table. Equality transfers this evidence through proxy roots.
A call of an unknown value slot supplies its Function skeleton; an unknown
type-level callee still waits for type evidence. This replaces the special case
for syntactically direct parameter calls and closes factory()(), callable
aliases and composed callbacks without requiring concrete call sites. The
published MIR retains their solved types and instances, not the scratch table.

Metadata joins compare represented-type structure without equating branch
witnesses. Equal represented types preserve TypeOf(T); different witnesses or
a Type branch produce Type. Never branches do not erase the surviving witness.
Mixed ordinary/metadata branches and attempts to fit broad Type metadata to a
specific TypeOf witness produce static conflicts. Codegen continues to select
the original metadata value, retaining its represented TypeId.

The actual inference-contracts query exposed an additional bottom bug:
equal-length ArrayLiteral terms were merged as positional type arguments,
retaining the first Never instead of the common live element. All literal
merges now use homogeneous element evidence, and Never supplies only a deferred
fallback. Both [stop(), 1] and [1, stop()] infer Array(Int), while all-bottom
arrays infer Array(Never); original stop() expression slots remain Never.

Validation: 49 type-resolve and 64 codegen tests pass. The actual
query/inference-contracts fixture now passes all 46 expected exports, with no
diagnostics. Its expectations use the current MIR surface rendering (Bool,
Option, Array/Dict parentheses and singleton tuple commas), rather than the
old expanded descriptor strings. The never_array requirement remains Int and
was fixed in the solver, not weakened in the checker. Full language acceptance
still fails elsewhere; checked-cast, tail-call, provenance and other semantic
gaps, as well as old-path removal, remain unfinished.
Logs: /tmp/mir-static-closure-final-types.log,
/tmp/mir-static-closure-final-codegen.log and
/tmp/mir-static-closure-final-language.log. No VM implementation or optimization
pass was added, and no performance measurement was made.

### Implicit closure schemes and local instances share the static graph (2026-09-11)

Unannotated closure-valued let/def declarations now participate in a resolved
SymbolId dependency graph. Recursive SCCs retain monomorphic slots. Other
references wait for the declaration's generalization boundary, then instantiate
its scheme using the existing type-argument and instance tables. Nested schemes
finish before their containing declaration escapes. No environment is cloned
and no legacy inference module is called.

Generalization visits normalized signature leaves in structural order, retaining
captured unknowns and solver-only restrictions. Owned numeric/not/ordered/member
constraints cannot become unconstrained generic parameters. Ordered comparisons
now retain an explicit Int/Float/String constraint. Synthesized type-only binders
receive deterministic appended SymbolIds and A/B/... display names; existing
resolved symbols and syntax references are not renumbered or re-resolved.

The synthetic module export record now preserves a declaration scheme instead
of creating a monomorphic use. This closes unused exported apply/select/wrap
signatures as well as functions with concrete call sites. Explicit type
applications wait for the same authoritative declaration outcome.

Local generic instances are completed statically, including enclosing binder
substitutions in their instance keys. Codegen allocates per-instance function
handles, emits the already-instantiated closures and captures referenced local
handles. Forward local definitions and repeated references preserve their
instance identity. There is no downstream substitution, implementation search,
VM change or optimization pass.

Validation: 46 type-resolve and 62 codegen tests pass. Tests cover Int/String
reuse, explicit arguments, dependency order, partial annotations, unused exported
schemes, captures, cross-module references, recursive/alias/constraint rejection,
stable existing symbol IDs, deterministic MIR dumps and readable signatures.
The full language runner passes test/type-inference. test/compiler-semantics
now executes 36 cases: 34 pass, checked_casts and tail_calls fail (the latter
hits the 1024-frame limit). This is not full language acceptance. The
query/inference-contracts fixture still exposes metadata branch joining and
nested callable-shape gaps such as factory()(); provenance and other semantic
contracts also remain open. Full legacy removal is unfinished.
Logs: /tmp/mir-generalization-final-types.log,
/tmp/mir-generalization-final-codegen.log and
/tmp/mir-generalization-final-language.log. No performance measurement was made.

### Propagation closes return evidence before mechanical branching (2026-09-11)

The type pass now records lexical propagation boundaries as stable HIR edges,
visible in MIR dumps and required by sealing. Ordinary blocks remain transparent;
nested functions own their boundary. Propagation constrains the native Option/
Result constructor identity, operand success slot, boundary result family and
directional Result error evidence. A similarly named user enum is not a builtin
family. Return context can also establish an unknown operand's family.

Never tails retain earlier error returns: the otherwise unconstrained success
slot completes to Never, producing Result(Never, E). Mixed families and
incompatible errors remain static conflicts. The boundary equality reports its
conflict at the contributing question-mark expression.

Codegen emits existing tag comparison, branch, payload access and Return
instructions. The failure branch returns the original operand register; success
reads the existing payload. No new VM instruction, host transfer, runtime type
inference or optimization pass is introduced.

Validation: 61 codegen and 42 type-resolve tests passed before the final
diagnostic-location/dump refinement and extra inference assertion; the final
propagation-filtered run passes all 9 tests, including 3 new-pipeline tests.
Actual test/check-result passes all 9 cases, including helper composition,
short-circuiting, codec checks and Never tails. The enum-constructor-context
propagation_context case also passes. The full language suite remains failing:
compiler-semantics is blocked by implicit generalization, while propagation
diagnostic contracts and check-result provenance remain open. Logs:
/tmp/mir-propagation-codegen.log, /tmp/mir-propagation-types.log,
/tmp/mir-propagation-final-tests.log and /tmp/mir-propagation-language.log.
No performance measurement was made. Whole-pipeline acceptance and removal of
the old compiler paths are still unfinished.

### Sequence spreads consume statically closed element slots (2026-09-11)

Array spreads constrain every contribution to the common element slot, including
shared empty arrays and nested contextual literals. Tuple spreads flatten the
operand's solved element slots and reuse ordinary tuple classification, retaining
heterogeneous elements and nominal target contexts without mixing type and value
worlds. Wrong containers, nominal tuple operands and incompatible shared element
evidence produce static conflicts.

Codegen mechanically emits existing MakeArray/MakeTuple and ConcatArrays/
ConcatTuples instructions, preserving authored evaluation order. It neither
infers element types nor introduces a VM change or optimization pass. MIR must
carry every solved type argument and implementation choice needed downstream;
missing information is a static-stage gap, not a codegen recovery opportunity.

Validation: 60 codegen and 41 type-resolve tests pass. The full language runner
reports test/array-spread-inference, test/tuple-spread and test/struct-update as
passing. The overall suite still fails: tuple-spread query, spread diagnostic
contracts and provenance cases remain open, alongside other migration gaps.
Focused tests cover shared/nested empty arrays, heterogeneous/empty tuples,
generic copies, nominal element contexts, invalid operands and failure order.
Logs: /tmp/mir-sequence-spread-codegen.log,
/tmp/mir-sequence-spread-types.log and /tmp/mir-sequence-spread-language.log.
No performance measurement was made.

### Record spreads preserve winners and declared generic identities (2026-09-11)

Record/StructUpdate spread constraints now collect field contributions in source
order and apply contextual types only to final winners. Explicit duplicate names,
unknown update fields and invalid spread modes remain static errors. Dict spreads
use the common element constraint, including contextual dictionary literals;
mixing dynamic Dict and nominal struct spreads is rejected. The record operation
constraints are consolidated in type-resolve/record-operations.rs.

Codegen evaluates every authored expression in order, including overwritten
values, and merges existing VM values with MakeDict/MergeDicts. Generic copies
and final target construction use already-solved IDs; no VM change is needed.
The overwritten-value failure test confirms that static winner selection is not
dead-code elimination.

The actual StructUpdate fixture exposed two additional assembly omissions.
An unannotated def following a generic decl erased symbol_generics, and the
execution graph excluded Decl-origin symbols from instance tasks. Both now
preserve and consume the original declared generic identity. Int/String uses of
the same declared generic copy function execute as separate solved instances.
Integer BitAnd/BitOr/BitXor also now map mechanically to their existing opcodes.
This does not implement implicit generalization.

Validation: 59 codegen and 40 type-resolve tests pass. The rebuilt CLI passes all
7 StructUpdate language tests, covering spread override order, nested contexts,
generic identity, precedence and overwritten failures. The full language suite
still fails other cases; its last aggregate run preceded the bitwise mapping,
and the final StructUpdate result was verified separately. Array/Tuple Spread,
provenance, implicit schemes, remaining syntax/schema and legacy removal are
still open. Logs: /tmp/mir-record-spread-codegen.log,
/tmp/mir-record-spread-types.log, /tmp/mir-record-spread-update.jsonl and
/tmp/mir-record-spread-language.log. No performance measurement was made.

### Named field projection and basic struct update use solved shapes (2026-09-11)

FieldProjection constraints read the source nominal member skeleton, substitute
generic payloads, validate selected/renamed fields and duplicate destinations,
then fit the projected record to the target nominal context. A projection used
directly as an update contribution keeps its partial record shape. Provisional
Record sources wait for annotation constraints to settle; after convergence,
uncontextualized sources/targets produce static diagnostics.

StructUpdate now keeps the left operand's nominal identity and checks each
contributed field against its existing skeleton. Supported contributions are
named structs, explicit record fields and projections. Nested field literals
receive their existing expected-type constraints; no new nominal type is guessed.
Spread contributions remain unsupported and are not counted as completed update
semantics. Codegen mechanically reads/projects/updates fields with existing
opcodes, applies construction checks and stamps the solved target TypeId. No VM
changes are included.

Validation: 58 codegen and 39 type-resolve tests pass. New tests cover renaming,
empty/repeated-source projections, generic/local annotated projections, chained
updates, nested contexts, immutable bases, generic identity, construction-check
rejection and invalid field/type/source contexts. The full language aggregator
reports test/struct-projection, query/struct-projection and query/struct-update
as passing. It still rejects the overall suite: projection/update provenance,
spread update semantics and several diagnostic contracts remain open. Logs:
/tmp/mir-record-operations-codegen.log, /tmp/mir-record-operations-types.log and
/tmp/mir-record-language.log. No performance measurement was made.

### Recovered containers do not invent result obligations (2026-09-11)

Parser recovery may retain a module block without a result expression. Type
generation no longer marks that missing result as required: the container slot
remains unfilled, while original syntax diagnostics prevent sealing. This is
not module-wide diagnostic suppression and does not manufacture a Unit type.
Recovered healthy declarations and independent type conflicts still solve.

Validation: the previously failing CLI parser-recovery case passes, as do all
38 type-resolve tests. A new mixed-error test retains a missing-FatArrow
diagnostic, a Known healthy export and a Conflicted bad expression without an
extra unknown-result diagnostic, and verifies seal rejection. Logs:
/tmp/mir-parser-fallout.log, /tmp/mir-parser-independent.log and
/tmp/mir-parser-types.log. Full CLI acceptance has not been rerun after this fix.

A read-only tally of the last language-suite output identifies common missing
rules: Spread (51 diagnostics), StructUpdate (21), FieldProjection (16), and
Propagate (13), alongside implicit generalization and other gaps. These are
diagnostic counts, not independent defect or failed-test counts; numerous
Unknown outcomes are downstream effects. Implement these shared MIR rules
rather than special-casing the individual fixtures. Legacy removal and full
semantic acceptance are still incomplete.

### Query preserves declaration binders and module outcomes (2026-09-11)

CLI definition/export rendering now uses the shared symbol_signature query.
The query follows the resolved declaration identity before reading its generic
binders, so selective imports, renamed reexports and wildcard imports retain
the source scheme. Type families display their solved parameterized skeleton,
e.g. `for(EntityId) TypeOf(Entity(EntityId))`, instead of reconstructing an old
function-shaped type constructor description. Namespace query records expose
their already-solved namespace TypeId and Known state. Tests were updated to
assert these MIR-based representations, rather than absence of namespace types.

Missing module roots now produce readable logical-name diagnostics. Query root
selection failures use the same JSONL diagnostic channel as resolve failures;
private standard-library roots remain inaccessible and report unknown builtin
module without exposing implementation paths.

Validation: 12 query-filtered CLI cases, 4 core query tests, 2 module-resolve tests
and 28 telora library/LSP tests pass. Full CLI acceptance now completes with 64
passes and 2 failures: parser-recovery diagnostic fallout and the aggregate
language suite. The language suite still contains many failing semantic cases;
the test count must not be read as only two remaining implementation defects.
Logs: /tmp/mir-query-cli.log, /tmp/mir-query-core-final.log,
/tmp/mir-query-modules.log, /tmp/mir-query-lsp-final.log and
/tmp/mir-query-full-cli.log.

Investigation confirms an independent, unresolved inference gap: unannotated
function declarations currently share their slots with all references, so
otherwise polymorphic Int/String uses conflict. Completing implicit schemes
requires solving declaration dependency components before admitting independent
reference instances, retaining constraints from captured outer slots and avoiding
generalization of unresolved numeric/member requirements. This must happen in
the static solver and preserve its Unknown/Conflicted results; cloning types at
calls or adding downstream inference would not satisfy the architecture. The
query changes above do not implement implicit generalization.

### Interpolation and generic trait dispatch close in MIR (2026-09-11)

Syntax lowering now expands interpolated expressions into ordinary
Display.display(value) calls. A compiler-generated hygienic import of the
std/fmt Display export participates in module and symbol resolution, including
when std/fmt itself contains interpolation. Raw text parts stay String nodes.
The normal trait and call constraints select the conversion and close its types;
codegen merely emits the resulting calls and the existing interpolation opcode.

Generic interpolation exposed a broader omission: concrete function instances
retained substituted node types but not their trait-member implementation
choices. GenericInstance now also records per-node implementation instance IDs,
derived during static materialization from already-proved concrete evidence.
Missing implementation evidence in a concrete instance produces a static
diagnostic. Codegen reads those IDs and includes their implementation dependencies
in compilation, including implementations used by checks or property providers.
It does not select implementations or substitute types downstream.

Validation: 37 type-resolve tests, 56 codegen tests and all 28 telora library/LSP
tests pass. New cases cover primitive/custom/generic Display, hygienic name
shadowing and static rejection without a Display implementation. The existing
display language fixture now passes, including property-driven formatting and
explicit implementation precedence. The minimal higher-order inference test
inventory now supplies a small std/fmt interface because its interpolation
actually participates in module resolution. Logs:
/tmp/mir-interpolation-types.log, /tmp/mir-interpolation-codegen.log,
/tmp/mir-interpolation-lsp.log and /tmp/mir-interpolation-display.jsonl.
Full language/CLI acceptance and legacy removal remain open. No VM change or
performance measurement is included.

### Proven property evidence lowers to a VM demand (2026-09-11)

The std/type-property evidence native now lowers to GetTypeProp with its two
metadata arguments. Its Property(P) bound is proved before sealing; unlike the
optional lookup adapter, it neither checks presence again nor wraps the value
in Some. The returned value remains in the VM heap, and a failed provider uses
the existing cached failure path. Codegen property dependency discovery now
includes this native entry, so its demanded provider tasks are installed.

Validation: 55 codegen tests pass. New coverage exercises a first-class evidence
function inside a bounded generic function, absent-property seal rejection and
provider failure propagation. The display language fixture now gets past the
previous unsupported property ABI and fails later at string interpolation:
MIR currently supplies the raw value instead of a statically selected Display
conversion to Fmt. That static elaboration gap remains open; the fixture is not
passing yet. Logs: /tmp/mir-property-evidence.log,
/tmp/mir-evidence-codegen.log and /tmp/mir-evidence-display.jsonl. No VM changes
or performance claims are included.

### Block bottom propagation is solved before tail fitting (2026-09-11)

Blocks now have an explicit static constraint over evaluated binding initializers
and the tail. A Never initializer makes the block Never, including discarded
expressions, explicit return, and `let x: Int = fail!(...)`; the annotation must
not hide the initializer's control-flow result. Otherwise the block takes its
tail type after initializer evidence is available. Fit constraints wait for this
decision instead of prematurely merging the block with a Unit tail or contextual
return type. Function bodies are not executed or inspected by a runtime fallback.

The changed scheduling also exposed indexing an ArrayLiteral before literal
normalization. Static indexing now accepts that existing provisional constructor
and supplies the same Int index and element constraints as Array.

Validation on the final change: 37 type-resolve and 54 codegen tests pass; the
unit-blocks language fixture passes all five cases, and the success-check
aggregate passes. Added cases cover discarded/bound Never, annotated bottom
initializers, early return, a declared Never-returning call, ordinary Unit tails,
and inferred/contextual types. Production codegen and VM are unchanged.
Logs: /tmp/mir-block-types.log, /tmp/mir-block-codegen.log,
/tmp/mir-unit-blocks-fixture.jsonl and /tmp/mir-block-success-check.jsonl.

A broader core run before the final initializer refinement completed with 486
passes and 71 failures, all reported in old module/semantic/module-id consumers;
the first failure is old native registration rejecting std/prelude.property.
This is not a green full-core acceptance result. These old consumer tests and
their retained semantics still need migration alongside legacy-path removal.
Log: /tmp/mir-block-core.log. No performance measurement was made.

### Local recursive closures consume resolved block identities (2026-09-11)

Local function references previously failed codegen because closures were emitted
before their own or mutually recursive bindings had registers. Blocks now reserve
function handles for resolved Def/Decl symbols whose closed type is Function.
Closures capture those handles; definitions install their generated functions
using the existing AllocFunc/SealFunc operations. Decl/Def pairs share the same
SymbolId and slot. Each block invocation allocates its own handles, so escaped
recursive closures retain the correct invocation's captured values. No name
lookup, type reconstruction or VM representation change is added.

Validation: all 53 codegen tests pass, including new cases for self recursion,
mutual recursion, declaration/definition pairing and independently escaped
closures with distinct captured values. The previously failing
explicit-boundary-types language fixture now passes all five tests through the
new CLI/VM pipeline. Logs: /tmp/mir-local-recursion-codegen.log and
/tmp/mir-local-recursion-fixture.jsonl. This does not close the other acceptance
gaps: in particular, block typing still loses Never from discarded statements
when it equates a block with its Unit tail. Broader generic, property/schema,
diagnostic/query and legacy-removal work remains.

### Newtype projection and unary lowering close acceptance gaps (2026-09-11)

The success-check aggregate exposed a missing static rule for the existing
newtype `.0` payload access. Projection now reads the declared newtype member
and instantiates its payload in the solver. Other positional indexes, record
projection and enum projection remain rejected. Existing tuple bytecode and VM
representation already support newtypes, so no runtime or codegen change was
needed for this rule. The full success-check aggregate now passes.

The language suite next reached a missing unary-expression codegen branch.
Unary lowering now emits existing negate/logical-not/bit-not instructions.
The solver retains the language's Bool-or-Int constraint for `!`; codegen selects
the instruction using the closed operand TypeId, including instantiated function
bodies. It does not use a dynamic operator-family fallback.

Validation: all 37 type-resolve and 52 codegen tests pass. Added execution cases
cover concrete/generic/nested newtype payloads, Boolean/integer `!`, negative
integers/floats and function-context inference; static rejection covers invalid
projections and floating-point `!`. Full CLI acceptance after the projection
fix completed with 60 passes and 6 failures. After unary lowering, the language
suite executes its final result aggregator but still fails many cases; it is
not accepted yet. Logs: /tmp/mir-projection-types.log,
/tmp/mir-unary-codegen.log, /tmp/mir-cli-after-projection.log and
/tmp/mir-unary-language.log. Remaining observed gaps include local recursive
bindings, property/schema ABI support, generic inference and diagnostic/query
contracts. No performance result is claimed.

### LSP protocol consumes MIR; cyclic aliases terminate (2026-09-11)

The LSP protocol now uses mir_workspace and MirQuery for diagnostics, hover,
definitions, references and completion. Its production Engine and old
WorkspaceSnapshot dependencies are removed. Generic signature display reads
the original binders and bounds. Incomplete-dot parser recovery retains the
receiver for the normal static passes; completion consumes that solved receiver
without a second name-resolution or inference path. Syntax errors still prevent
sealing. LSP tests verify that panic and division-by-zero initializers are not
executed while serving static diagnostics.

Broad CLI acceptance exposed nontermination when a generic transparent alias
and a concrete alias expand each other. Before constraint generation, an
iterative SCC traversal of resolved alias identities now marks cyclic aliases
Conflicted. Nominal definitions remain recursion boundaries, and unrelated
types continue solving. The original mixed-cycle fixture now terminates with
diagnostics; the aggregate diagnostic graph also finishes. This is a termination
fix, not a general performance measurement. Diagnostic fallout remains to be
refined: the mixed-cycle fixture still includes dependent Unknown diagnostics.

Validation after the fix: 36 type-resolve tests, 51 codegen tests, all 28 telora
library tests (including 23 LSP tests), and the CLI UTF-8 source-position test
pass. Earlier parser and MIR query checks passed (8 and 4 tests). The earlier
full CLI run had 59 passes and 7 failures, including the stopped nonterminating
language suite. Source-position handling and alias termination are now fixed;
full acceptance has not yet been rerun. Remaining gaps include diagnostic
suppression, query contract differences and language fixture type constraints.
Legacy core compiler removal is still open.

Codegen remains mechanical: it consumes sealed bindings, types and generic
instances. Missing static information must be completed by the MIR passes;
neither downstream inference nor an optimization pass is part of this assembly.

### Versioned editor workspace owns one MIR snapshot (2026-09-11)

Added telora::mir_workspace, independent of Engine, WorkspaceSnapshot and the
VM. A rebuild captures document versions, obtains canonical names from the
shared inventory, and solves the selected module plus all open documents as
roots in one graph. Overlays replace source text by module identity; imported
and explicitly opened copies of the same module remain one module. Invalid
programs retain the completed static passes and their diagnostic graph without
requiring seal or any user-code execution.

Snapshots own Mir and a physical-path-to-original-ModuleId lookup, and expose
the shared MirQuery facade. The version clock and cancellation checkpoints guard
publication: a stale/cancelled rebuild cannot replace the last snapshot; older
snapshots remain immutable and reject queries against newer contexts. Rebuilds
are currently synchronous within each static pass, with cooperative checkpoints
around input capture and the solve; this is not intra-pass preemption.

Moved Inventory from binary-only code into the telora library so CLI and LSP
assembly use the same host input implementation. Existing command consumers now
import that shared module. The bounded read helper also has one implementation.
Editor document selection allows private catalog modules; test discovery retains
its symlink rules and uses the declaring crate's identity, including when multiple
open roots reuse the same catalog entries.

Validation: 3 new workspace tests pass (overlays/multiple roots/close-to-disk,
cancellation during a yielded build and stale publication, transactional document
versions/private test roots); 2 inventory tests, 17 static-MIR CLI tests and 7 test
CLI regressions pass. Logs: /tmp/mir-workspace.log,
/tmp/mir-workspace-inventory.log, /tmp/mir-workspace-cli.log and
/tmp/mir-workspace-test-cli.log. No full-LSP or performance claim.

The LSP protocol handler still imports the old Workspace/WorkspaceSnapshot. The
next step is to replace those imports and wire request/diagnostic handling to this
workspace and MirQuery, then remove its old dependencies. Remaining language
coverage, broader old-compiler removal and final acceptance are still open.

### Shared read-only MIR queries for CLI and LSP assembly (2026-09-11)

LSP audit finds that Workspace rebuild still calls Engine recovery and produces
a separate WorkspaceSnapshot with remapped Definition/Reference/Type IDs. That
route cannot become the new frontend's authoritative information graph.

Added mir_query::MirQuery, a borrowed facade over the actual Mir. Definition
locations, references, targets, expression types, symbol types and member lists
return original SymbolId/HirId/TypeId identities and existing resolve/type states.
The facade owns no parallel semantic graph and has no inference, seal or VM
dependency. Unresolved/Conflicted references yield no target; queries do not
search names again to repair them. References emitted twice by module export
lowering are deduplicated by their actual source location for reference lists.

Member queries read namespace export IDs, structural record argument IDs and
precomputed nominal layouts. Generic member types are not substituted during
querying; if a concrete layout is absent, the query does not invent one. The CLI
now shares type rendering, definition/export type reads and reference enumeration
with this facade. Field reference ranges cover the member name, not its receiver.

Validation: 4 new core query tests pass, covering cross-module rename/import
identity, shadowing, unresolved/duplicate conflicts, namespace fields and concrete
generic layouts; 2 static-MIR CLI query regressions pass. Logs:
/tmp/mir-query-core.log and /tmp/mir-query-cli.log. This is a query foundation,
not a completed LSP migration or a performance result.

Next assembly: the LSP versioned workspace must own a Mir built from the source
inventory and document overlays, retain cancellation/stale-publication checks,
and serve hover/definition/references/completion from this facade. Existing
Workspace/WorkspaceSnapshot/Engine LSP dependencies remain until that switch;
the broader legacy compiler removal and full acceptance remain open.

### CLI test switches to sealed MIR; old test loader removed (2026-09-10)

The test command now enters Inventory, module/symbol/type resolution, TestPlan,
codegen, linking and Vm.test_linked. Inventory supplies physical module paths to
the fixture host; runtime data diagnostics use canonical module identities.
The existing telora.test/v2 formatter consumes the new report, preserving source
labels, nested fixture trails, factory notices and terminal-abort status.

Removed module/test.rs and Engine::test_with_resolver, the old deferred-test
runner tests and its unused WorkWorld constructor. Their shared-quota, expansion,
input-cache and prepare-before-factory coverage is now in the solved VM tests.
The CLI's last non-LSP Engine factory is removed. No old-path fallback remains
for test. LSP still uses Workspace/WorkspaceSnapshot/Engine and remains a gate.

CLI coverage exposed Dict field access missing from the static member pass.
The pass now records DictField and its element type; codegen mechanically emits
the existing field-read instruction. Runtime representation is unchanged.
Shared data validation now retains structured diagnostics so multiple malformed
data modules are reported before bootstrap or user initializer execution.

Tests were updated to exercise direct Test exports and demand semantics:
syntactic import cycles alone are accepted, actual evaluation cycles fail,
independent failed initializers are collected, and unread globals stay lazy.
The controlling RFC now explicitly documents the superseded eager test lifecycle.
New CLI fixtures cover helper-relative JSON/YAML/TOML, nested factories, warnings,
bad-data siblings, real crate escape rejection and malformed module data.

Validation: all 7 test-command CLI tests, 17 static-MIR CLI regressions (including
check/eval/eval-with/run/serve), 48 VM tests and 51 codegen tests pass. VM coverage
also verifies bootstrap allocation failure and fixture materialization allocation
failure produce aborted reports at the correct boundary. Logs:
/tmp/mir-test-cli-switch.log, /tmp/mir-test-switch-static-cli.log,
/tmp/mir-test-switch-vm.log and /tmp/mir-test-switch-codegen.log.
No full-suite or performance claim. Remaining assembly includes LSP, unresolved
language/codegen coverage and removal of the broader legacy compiler modules.

### Fixture expansion uses the solved test session (2026-09-10)

Vm.test_linked now expands with_fixtures through an explicit depth-first work
stack. Each group resolves, reads and validates all immediate inputs before any
factory executes, caching source text once per host key within that group.
Validated data is materialized directly into the existing Work heap; factories,
returned Test witnesses and captured fixture values retain their original VM
handles. There is no world import or user-data graph copy between cases.

TestPlan records the factory input TypeId from the sealed native module 33
with_fixtures ABI signature. Runtime materialization consumes that ID without
searching for a type named Value or inferring a factory signature. Neutral
TestHost/TestLimits/TestSource now live with the shared test protocol, together
with TestContext's host and physical module paths.

Reports retain nested fixture indexes, source labels, validation diagnostics and
factory notices. Recoverable factory/data failures allow siblings and later
exports to run; terminal failures abort. Expansion count, nesting depth, aggregate
fixture retention and VM allocation charges bound the work. Synthetic fixture
sources use the existing @test-ctx namespace and preserve individual occurrence
locations even when the input text was read from the same cached source.

The ordering regression exposed a missing Debug lowering. Codegen now evaluates
the operand, emits the existing Debug instruction with source metadata and
returns the same register. No inference or optimization was added to codegen.

Validation: 47 VM tests and 51 codegen tests pass; cargo check -p telora passes.
Four new fixture regressions cover input caching/provenance, invalid data with
successful siblings, nested depth-first expansion, recoverable factory failures,
expansion/retention limits, prepare-before-factory ordering and factory warnings.
Logs: /tmp/mir-fixtures-vm.log, /tmp/mir-fixtures-codegen.log and
/tmp/mir-fixtures-cli.log. No full-suite or performance claim.

The core runner's fixture gate is removed. CLI test still uses its old route;
the next assembly step is connecting the inventory and v2 report formatter to
this runner and deleting that route. LSP and broader old-pipeline removal remain.

### Direct test cases execute in one solved VM session (2026-09-10)

Vm.test_linked consumes the compiled test bootstrap and static TestPlan, loads
data before initialization, then demands each selected export and calls its
deferred thunk. The same Work heap moves through successful and failed calls;
there is no per-case world relocation or cloning of user graphs. Test results
and diagnostics are the only session output.

Intermediate test calls do not attempt session publication. Failed lazy tasks
retain their cached failure while unrelated later cases can execute. All existing
non-test callers still enforce their previous publication boundary. The runner
distinguishes export initialization errors, expected recoverable thunk failures,
message mismatch and terminal quota failure; terminal failures abort even under
should_fail. Native test description data now lives in independent test_protocol,
shared by native constructors and runners rather than owned by the old loader.

Validation: all 40 VM tests and 50 codegen regressions pass; cargo check -p telora
passes. New cases cover repeated cached dependency failure, expected failures,
success afterward, unexpected success, initializer failure followed by another
case, and quota exhaustion that must abort. Logs: /tmp/mir-test-session-vm.log,
/tmp/mir-test-session-codegen.log, /tmp/mir-test-session-cli.log and
/tmp/mir-test-session.log. No performance/full-suite claim.

Fixture expansion remains explicitly unsupported in this new runner, with an
aborted report rather than an old-path fallback. telora test has not switched;
fixture context, expansion limits and CLI report integration must land first.
The broader migration, LSP integration and old-pipeline removal remain open.

### Static test discovery and a deferred test-session bootstrap (2026-09-10)

Command audit confirms check/query/eval/run/serve enter the new pipeline, while
test still discovers exports after old ModuleGraph/WorkspaceBuilder execution;
the LSP WorkspaceSnapshot path also remains a migration gate.

The new independent test_plan module discovers direct monomorphic Test exports
from SealedMir using native identity (module 33, slot 0). It retains export names,
resolved SymbolIds, target SymbolIds, TypeIds and locations in deterministic name
order. Same-named user types and functions returning Test are not test values.
Reexports preserve their resolver-established target without another name lookup.

compile_tests returns that plan and a Tests(module) bootstrap. The bootstrap
installs the existing lazy global/property/checker tasks and returns Unit without
demanding test exports or invoking thunks. Both the static planner and codegen
remain independent of the old module loader/test runner implementation.

Validation: four discovery/bootstrap tests and 50 codegen regressions pass;
cargo check -p telora passes. A Test-typed failing initializer and a failing test
thunk prove neither runs during discovery/bootstrap. Logs: /tmp/mir-test-plan.log,
/tmp/mir-test-root-codegen.log and /tmp/mir-test-plan-cli.log.

This does NOT switch telora test yet. Per-case execution, fixture expansion,
session quotas and diagnostic collection must consume the new session before the
CLI can replace test_with_resolver and its old loading path. No performance or
full-suite acceptance claim; all broader migration/removal gates remain open.

### Contextual tuple literals retain per-element completion evidence (2026-09-10)

Value tuple syntax now starts as a provisional TupleLiteral, distinct from an
already fixed Tuple type. Contextual tuple constraints fit each source element
against the corresponding target, retaining Unchecked completion adjustments
instead of unifying the candidate's identity with T. Generic instantiation keeps
the existing refinement edge alive for provisional tuple syntax as well.

The static literal-finalization scan normalizes remaining tuple literals to Tuple;
seal rejects any provisional ArrayLiteral/TupleLiteral in the published type
arena. Tuple projection can consume provisional child slots during solving.
No codegen inference or new runtime operation is needed.

Validation: 35 type-pass and 50 codegen tests pass; cargo check -p telora passes.
Five new execution cases cover tuple assignment, function argument/return,
nested tuple construction and checker rejection. Static assertions verify the
candidate retains Unchecked identity, the element target is T, and no provisional
literal constructor escapes normalization. Logs: /tmp/mir-tuple-static.log,
/tmp/mir-tuple-codegen.log and /tmp/mir-tuple-cli.log.
No performance/full-suite claim. Broader recursive/composed inference coverage,
generic evidence/property support, command/LSP integration and old-pipeline
removal remain required before the overall migration can be called complete.

### Branch joins retain directional construction evidence (2026-09-10)

When a join includes Unchecked values, the static pass waits for branch evidence
and uses a checked branch (or the known contextual target) as the result type.
Each candidate branch gets a value adjustment instead of merging its source slot
with T. Match joins now name their actual arm expressions, the same nodes emitted
by codegen, so adjustments cannot be stranded on administrative MatchArm nodes.
Codegen needs no new inference or branching rule for these conversions.

Validation: 49 codegen regressions and the existing 33 type-pass tests pass; an
additional static test passes with explicit assertions that the original candidate
remains Unchecked(T), the join is T, and exactly one adjustment is recorded. Eight
execution cases cover if/match, branch-order reversal, unselected invalid
candidates, selected rejection, contextual targets and generic checker instances.
Logs: /tmp/mir-branch-codegen.log, /tmp/mir-branch-static.log and
/tmp/mir-branch-specific.log. No performance or full-suite acceptance claim.

Tuple contextual completion and broader recursive join coverage remain open,
alongside generic evidence/property work, command/LSP integration and removal of
the old pipeline. The overall migration is not complete.

### Unchecked metadata shares the solved owner layout (2026-09-10)

TypeImage.layout follows Unchecked's canonical owner ID and returns the owner's
existing layout by reference. It does not duplicate member arrays or manufacture
new body IDs. TypeDesc.kind reports Ref; resolve and fields consume the shared
body. Applied generic members and recursive checked field identities remain
unchanged. Dyn field access follows the same owner relation while the package
retains its original Unchecked descriptor and payload handles.

Validation: 48 codegen and 14 solved VM tests pass. New cases use an always-failing
checker to prove that metadata/field observation does not complete the candidate.
A detached-image assertion verifies pointer equality between owner and Unchecked
layouts. The Dyn regression verifies original array payload-handle reuse for
unchecked field access. Logs: /tmp/mir-unchecked-metadata.log,
/tmp/mir-metadata-codegen.log and /tmp/mir-metadata-vm.log.
No performance/full-suite claim. Contextual tuple/branch completion, remaining
generic evidence/property support, command/LSP migration and old-pipeline removal
remain open.

### Unchecked construction and explicit MIR completion boundaries (2026-09-10)

Unchecked(T) now admits named-field Struct candidates, exposes their fields and
preserves distinct identity without running the outer checker. Nested Unchecked
applications collapse; direct and instantiated non-Struct applications produce
static diagnostics. A pending generic owner skeleton remains a shape-equality
constraint until its definition is available instead of prematurely conflicting.

Implicit completion is recorded separately in MIR value_adjustments. The source
slot stays Unchecked(T), while each consuming boundary names its target T. Generic
instances retain concrete adjustment targets. Seal verifies both source and target
records; missing instance adjustments are rejected rather than silently skipped.
Codegen emits the existing checker call followed by StampType, preserving the
candidate's heap handle. Return-slot adjustments are emitted at the closure return
boundary. Record construction also stamps its statically determined identity.

Covered paths include bindings, function arguments, implicit returns (including
generic checker instances), record fields, contextual array literals and cast!.
Dyn packaging keeps checked and unchecked identities separate. Wrong nominal
targets remain conflicts. The VM test verifies that completing a candidate keeps
the original record handle and field provenance while changing only its TypeId.

Validation: 33 type-pass tests, 47 codegen tests and 14 solved VM tests pass;
cargo check -p telora passes. Logs: /tmp/mir-unchecked-static.log,
/tmp/mir-unchecked-codegen.log, /tmp/mir-unchecked-vm.log and
/tmp/mir-unchecked-cli.log. No performance measurement or full-suite claim.
Remaining boundary coverage includes tuple/branch contextual conversions and
Unchecked metadata consumers; remaining generic evidence/property work, command
and LSP integration, and removal of the old pipeline are still required.

### CheckedCast uses solved IDs and VM construction tasks (2026-09-10)

The static pass now gives cast! its Result(Target, String) contract without
executing the target type expression. Codegen mechanically emits CheckedCast
with the closed source and target TypeIds, including inside generic instances.
The solved VM path is independent of the old cast native/TypeDescriptor pipeline.

A flat work stack first validates the complete representation without executing
checkers, then refines nominal boundaries and invokes their lazy checker tasks.
Shape mismatch returns Err(String) with a root-relative path; construction
rejection raises the original BlameError, and checker execution failures propagate.
Existing nominal identity is preserved: same-type casts skip checks, while a
different nominal source is not treated as an unowned record. No parsing, numeric
conversion or codec property interpretation is performed.

Record/Dict, Array, Tuple/newtype, Option and Result traversal uses image child IDs.
Unchanged values retain their handles and provenance. Refined record roots reuse
the underlying dictionary; when nested type stamps change, only necessary parent
containers are rebuilt. Exact static identity does not add redundant scalar
stamps that would force otherwise unnecessary container copies.

Validation: 46 codegen tests and 13 solved VM tests pass; cargo check -p telora
passes. Added execution cases cover nested checks, array/Option/Result/newtype
casts, nominal isolation, generic bodies, mismatch-before-check ordering and
failure propagation. VM tests assert original record/String handles, nested
nominal stamps and input locations. Logs: /tmp/mir-cast-codegen.log,
/tmp/mir-cast-vm.log and /tmp/mir-cast-cli-check.log.

Unchecked construction and implicit completion are still open, so their cast
entry cases are not yet end-to-end supported. Remaining command/LSP migration,
legacy removal and full corner-case/performance acceptance remain required.

### Generic construction checker bodies close in the static graph (2026-09-10)

ConstructionCheck now retains template/concrete status and, for applied owners,
the GenericInstanceId containing the checker's normalized node types and reference
edges. The template HIR remains shared. ExecutionGraph admits concrete checker
contracts only; codegen selects their existing instance context and mechanically
emits the checker thunk. The generic-checker codegen rejection gate is removed.

Static instance materialization and nominal layout expansion now close together.
New applied owners discovered through member layouts or instantiated function
bodies receive checker contracts; generic calls inside those checkers extend the
same instance worklist. Layout expansion resumes at its previous array cursor.
Seal checks each applied checker signature against its instance node-type table.
This performs no Telora execution and adds no downstream type substitution.

Validation: 32 type-pass tests, 45 codegen tests and 12 solved VM tests pass.
New cases cover Int/String applications sharing checker syntax, generic calls in
checker bodies, owners discovered only through nested member layouts, owners
produced by generic functions, normal rejection, and Struct/newtype/variant/codec
execution. Logs: /tmp/mir-generic-check-static.log,
/tmp/mir-generic-check-codegen.log and /tmp/mir-generic-check-vm.log.

This closes generic owner-parameter specialization for construction checkers,
not every generic feature: instance-specific assumed trait/property evidence,
generic property providers and local declarations capturing outer type parameters
remain open, as do CheckedCast/Unchecked paths and the remaining integration and
legacy-removal gates. No performance/full-suite acceptance claim.

### Codec and text parsing invoke solved construction checks (2026-09-10)

The solved codec and string parser now invoke the existing lazy checker tasks
at Struct, newtype and payload-variant construction boundaries. Checker calls
use original VM value handles; codec rejection returns the original BlameError
handle without repackaging or registering a VM failure. Untagged decoding treats
check rejection as a candidate mismatch, while checker execution failure escapes
the trial and preserves the cached failure. Checker calls and standalone parse
rejection retain the checker declaration as their rule location.

Codegen's temporary codec/parser rejection gate is removed. This adds no type
inference or specialization to codegen: it consumes sealed contracts, and generic
checker specialization remains an explicit missing static capability. Other open
items include CheckedCast/Unchecked paths and richer all-failed untagged diagnostics.

Validation: 44 codegen tests and 12 solved VM tests pass, including rejection,
untagged fallback/ambiguity, execution failure propagation, original BlameError
identity, string input identity and provider failure caching. Logs:
/tmp/mir-construction-final-codegen.log and /tmp/mir-construction-final-vm.log.
No performance or full-suite acceptance claim; overall migration remains open.

### Ordinary construction invokes solved checker tasks (2026-09-10)

ExecutionGraph now assigns a lazy task to each construction checker, indexed by
(owner TypeId, construction site). Codegen installs a thunk for the checker
expression and admits its referenced globals/native dependencies before emission.
Struct literal construction, newtype constructors and payload-variant constructors
emit ordinary Demand/Call/Result-branch/Raise instructions using the sealed
contract. No new inference or runtime type recovery is introduced.

The VM demand table caches the checker function, not each validation result.
Each construction calls it; Err(BlameError) is raised at the construction boundary.
Regression cases cover all three supported boundaries, referenced global checker
dependencies, rejection diagnostics, and two rejected values followed by an
accepted value through the same cached checker. All 43 codegen tests pass:
/tmp/mir-check-execution-codegen.log.

Generic checker specialization and codec/parser invocation are still pending.
Those combinations explicitly fail codegen rather than silently omitting checks;
the earlier blanket rejection of all construction-check graphs has been removed.
The graph lookup is now available for VM codec/parser consumers. This checkpoint
does not close the overall migration or claim performance/full-suite acceptance.

### Construction checks have independent static contracts (2026-09-10)

MIR lowering now recognizes the existing intrinsic @check syntax separately from
property decorators. Only the checker expression participates in ordinary symbol
resolution; no fictional prelude check provider is resolved. The static pass
retains owner TypeId, construction site, checker HirId and complete signature in
construction_checks, exposed in MIR dumps and validated at seal.

Struct checkers take Unchecked(owner); newtype and payload-variant checkers take
the payload type. Their result is Result((), BlameError), with BlameError selected
by its native identity. Field access through Unchecked consumes the owner's
existing member skeleton. Duplicate checks, unsupported boundaries, malformed
arguments and incompatible result types are diagnosed statically. Even a checker
body containing fail! can be solved/sealed without execution.

Validation: all 31 type-pass tests and 43 codegen tests pass. New cases cover four
valid boundary/signature contracts and eight invalid declarations. Logs:
/tmp/mir-check-contracts.log and /tmp/mir-check-contract-codegen.log.
This is static admission only: execution installation, generic application
specialization and constructor/codec/parser invocation are next. Codegen currently
explicitly rejects graphs containing construction checks, preventing silent
omission while this handoff is incomplete. No full-suite or performance claim;
the overall migration gates remain open.

### Solved regex preparation, string parsing and codec text decode (2026-09-10)

regex.prepare now validates capture names and optionality against the already
applied Struct body and uses the execution graph's property-presence records for
nested parse capability. It returns the original compiled Regex handle. Nested
ParseBy providers are not eagerly executed during preparation. Native callbacks
get read-only access to the admitted TypeImage/ExecutionGraph for this check.

string.parse uses a flat VM task stack carrying the original input Val, absolute
capture ranges and solved member IDs. Nested ParseBy demands suspend/resume the
same stack. Scalars and Struct/Option results materialize directly in the VM;
whole-input String captures reuse the input handle, while proper substrings are
new output strings. There is no Rust ParsedValue tree or legacy descriptor path.
Syntax/scalar mismatches return Result errors; provider failures propagate the
cached FailureId. String parse native calls admit provider dependencies in codegen.

codec DecodeByParse now calls this parser through a native continuation. Parse
mismatches become BlameError data failures and can participate in untagged
alternative trials; provider failures never become candidate mismatches. Paired
decode/encode marker validation remains required. Nested Service/Endpoint values
roundtrip through the solved text decoder and prepared display encoder.

Validation: all 42 codegen tests, 11 codec regressions, and two string-parse
tests pass (overlapping selections). Coverage includes required/optional and
missing/extra captures, unsupported member capability, nested lazy parsing,
non-finite Float rejection, untagged ambiguity/fallback, cached provider failure,
and original String handle/literal location retention in captures and ParseError.
Logs: /tmp/mir-text-parse-codegen.log, /tmp/mir-text-decode.log,
/tmp/mir-text-parse-handles.log. No full-suite or performance claim.
Construction @check execution is still pending, including how its rejection
participates in parse/untagged decoding; schema, generic evidence, remaining
consumers and complete old-pipeline removal also remain open.

### Codec text encoding resumes through prepared DisplayBy (2026-09-10)

The solved encoder now honors paired DecodeByParse/EncodeByDisplay markers,
demands the VM-cached DisplayBy property, and calls its prepared display function
with a Dyn wrapper around the original value and solved owner ID. A native
continuation renders the returned Fmt into a new Value.String and resumes the
same encoder stack. Nested structs and repeated array elements use this path.
Text encoding takes precedence over structural rename/untagged options, matching
the existing bridge contract. Missing marker partners or DisplayBy are errors.

Rendering measures and charges the output size before allocating text, preserves
native allocation/stack failure categories, and forwards the codec rule boundary
through property and display calls. Property providers are cached; display(value)
is an ordinary call per input. Tests distinguish a failed provider (one FailureId,
one initial diagnostic, no repeated diagnostic) from two failed display calls
(two FailureIds and one diagnostic per call, without poisoning the provider).

Ten codec regressions pass, including nested Service/Endpoint text encoding,
incomplete contracts, array continuation, existing handle-sharing checks, and
the two failure lifecycles. Log: /tmp/mir-text-encode.log. No full-suite or
performance claim. DecodeByParse execution still awaits solved regex preparation
and parsing; construction checks, schema, generic evidence and legacy removal
remain part of the overall migration.

### Static body IDs and prepared DisplayBy execution (2026-09-10)

Nominal applied layouts now include a structural body TypeId allocated by the
static pass. Struct bodies reuse Record; Newtype and Enum body constructors
retain child IDs in the same arena, with enum payload-presence flags. Recursive
edges retain their nominal owner IDs. Sealing validates body/member ID bounds.
TypeDesc resolve returns this existing ID rather than building descriptors in VM.
Kind, children, fields, variants and opaque_name consume the immutable image;
nominal TypeDescKind/FieldDesc/VariantDesc results use the native signature's
result IDs. Property values are not needed for any skeleton observation.

A real @fmt.display_by("{host}:{port}") provider now prepares and executes through
the new chain, rendering localhost:8080 after dropping MIR. This exposed two
assembly gaps, now fixed: interpolation text parts were untyped auxiliary nodes
(they are now ordinary String HIR nodes), and a decl followed by def was mistaken
for an externally supplied binding (task generation now reads the final binding).
Codegen emits the existing interpolation instruction mechanically; it does no
type inference. The unused untyped Text HIR variant was removed.

Validation: all 37 codegen tests and 29 type-pass tests pass, including recursive
generic body closure, direct decl/def and interpolation, metadata observations,
and the real prepared display. Logs: /tmp/mir-type-desc-{codegen,static}.log.
A broader solved_ filter also selected a still-legacy semantic completion test,
which fails on obsolete std/prelude native property registration; it is not
included in these passing totals (/tmp/mir-type-desc-regressions.log).
No full-suite/performance claim. Codec parse/display bridging, regex preparation,
construction checks, schema, generic evidence and old consumer removal remain.

### Dyn member observations consume applied IDs (2026-09-10)

Text codec integration exposed a prerequisite: std/fmt's prepared display uses
indexed Dyn member access. The solved Dyn path now supports indexed fields and
variants, named fields/field lists, Array/Tuple/newtype items, variant tag/payload
and kind. Child witnesses come from the existing TypeImage layout or container
arguments. Nominal member indices follow declaration order; value access selects
the declared field name, independent of record construction order.

Dyn wrappers keep original payload handles. An explicit VM test checks that both
generic struct-field and generic enum-payload access retain the original Array
handle after MIR is dropped. Native dyn.kind receives its compiled function
signature and stamps the returned ValueKind with the signature's result TypeId;
otherwise equality with the statically typed enum constant loses nominal identity.
No name-based type discovery or runtime type substitution is involved.

Validation: three solved-Dyn tests pass (including ten structural observation
fixtures), and all 35 codegen tests pass. Logs: /tmp/mir-dyn-members.log and
/tmp/mir-dyn-codegen.log. This is a dependency completed for text codec, not a
claim that parse/display bridges are complete: solved TypeDesc observations and
regex preparation still need migration, alongside the remaining overall gates.

### Untagged decode uses VM-local alternative frames (2026-09-10)

The solved decoder now evaluates untagged enum alternatives from the already
applied member layout. A task-stack boundary retains output depth, next member
and successful Val handles. Only one matching alternative is accepted; zero or
multiple matches return a decode error, including ambiguous nullary alternatives.
Nested data mismatches discard candidate output handles without copying types,
input graphs or VM state. Pending frames survive lazy property suspension.

Property execution failures propagate through the existing failure channel and
are never treated as candidate mismatches, even after an earlier alternative
succeeded. The failure-cache regression covers this case and repeated calls.
Seven codec regressions and all 34 codegen tests pass. Logs:
/tmp/mir-decode-untagged.log and /tmp/mir-untagged-codegen.log.
No performance or full-suite claim. Parse/display bridges, construction checks,
schema, generic evidence and remaining consumer/legacy migration remain open.

### Solved decode and dictionary handle reuse (2026-09-10)

codec.decode now consumes the native instance signature and TypeImage member
layouts directly. Supported shapes include primitives, Option, Array/Tuple,
Record/Dict, nominal structs/newtypes and externally tagged recursive enums.
The flat decode stack retains VM handles across lazy property demands; rename_all
works for struct fields and enum variants, with duplicate external names rejected.
Missing optional fields become None; malformed input returns a BlameError with
the original offending Value handle and a field/index path.

Dict encoding and decoding preserve the input ShapeId and scalar payload handles.
The successful decode path does not invoke the old descriptor/check machinery.
Six codec regressions pass, including generic recursive layouts after dropping
MIR, renamed members, extra-field/type mismatches, shared dictionary shapes and
String payloads, and failed encode/decode properties cached with one diagnostic
on the first demand and none on repetition. Log: /tmp/mir-decode-properties.log.

Still pending: untagged decode alternatives, parse/display bridges, schema,
construction checks and generic property/evidence specialization. Unsupported
rules explicitly fail; this checkpoint is not full migration or a performance
claim. Codegen remains mechanical: all type specialization belongs to MIR.

### Solved encode unblocks real SQLite run/serve flows (2026-09-10)

codec.encode now reads its source TypeId from the compiled native signature and
walks TypeImage's applied member layouts. Scalars, arrays, tuples, records,
Option, nominal structs/newtypes/enums, rename_all and untagged encoding use this
path. A flat work stack retains VM Val handles; existing String payloads and
already-Value inputs are shared, not copied through host/world conversion.
Fuel and heap allocation use the session quota.

Nominal codec properties are requested through the VM's existing demand table.
The encoder suspends for an uncomputed provider and resumes after the same task
is cached. Property failures reuse FailureId. Diagnostic scopes now remember
their incoming demand depth and fail/cache only inner tasks when catching an
error, preserving outer computation. Repeating a failed property query returns
Err without another diagnostic or provider execution.

This exposed a static Never bug: function compatibility had unified an annotated
return skeleton with a Never-returning body. Function return fitting is now
directional; bottom is used for still-unknown result slots only after other
evidence reaches a fixed point. A provider whose body only fail!s retains its
declared property result and seals without executing it.

Validation: all nine run_ CLI regressions now pass, including the three SQLite
helpers previously blocked by codec. The concurrent SQLite serve regression
passes. Also passed: 32 codegen tests, 29 type-pass tests, 17 static-MIR CLI tests,
and four codec regressions covering recursive/renamed layouts, preserved input
handles, and one cached failure with one initial/zero repeated diagnostics.
Logs: /tmp/mir-codec-{run,serve}-cli.log, /tmp/mir-encode-{codegen-all,type-all,static-cli}.log,
and /tmp/mir-solved-encode-regressions.log. No full-suite or performance claim.

Remaining codec gaps include decode/schema, text display/parse bridges, Dict and
other unsupported constructors, plus generic property/evidence specialization.
Unsupported paths remain explicit errors; there is no old descriptor fallback.
Remaining consumer migration and complete legacy removal are still required.

### Applied nominal member skeletons belong to MIR (2026-09-10)

The static type pass now builds a TypeId-indexed member-layout table after
generic instance elaboration. It applies each nominal definition's solved
arguments to its member payloads, interns resulting types in the same arena,
and follows newly discovered member types until the table is closed. Recursive
edges retain the original owner TypeId. None denotes a nullary variant, never an
unknown field type; unknown member slots produce static diagnostics.

TypeImage imports this flat table and exposes layout(TypeId) as an array lookup.
No generic parameter substitution is delegated to VM/native consumers. Sealing
requires table coverage of the final type arena. Static member expansion has a
65,536-type resource bound; exceeding it diagnoses and prevents publication.

All 28 type-pass tests and 30 codegen tests pass. A new test checks Tree(Int)
and Box(String), their concrete payloads, nullary variants, and recursive member
TypeIds after dropping MIR. Logs: /tmp/mir-applied-layouts.log and
/tmp/mir-applied-layout-codegen.log. Solved codec consumers and instance-specific
property/trait evidence remain incomplete. No performance/full-suite claim.

### Native instances carry their statically selected signatures (2026-09-10)

Concrete native instances now use execution tasks as ordinary generic functions
do. Codegen emits a native relocation with the instance's full signature TypeId;
the linker installs this inline metadata as the final native closure capture.
Existing opaque native ABI captures remain at their original indices. Generic
native templates are not separately linked as executable closures.

CallContext::solved_signature exposes this compiler-provided identity to native
consumers without inspecting argument values. A regression invokes a native
through a generic forwarding function with Int and String and returns only its
captured signature, without reading either payload. This establishes the native
type-evidence ABI; solved codec encode/decode/schema are still pending.

All 30 codegen tests and 17 static-MIR CLI tests pass, including real serve
request diagnostics and opaque fmt captures. Logs:
/tmp/mir-native-instance-signatures.log and /tmp/mir-native-instance-cli.log.
No full-suite or performance result is claimed. Next static work must provide
fully applied nominal member skeletons so codec never substitutes generic
member types at runtime; property values remain VM-owned lazy queries.

### Codegen consumes static generic instance bodies (2026-09-10)

Concrete generic declaration instances now have execution graph task IDs. Codegen
emits their shared HIR using the instance's node TypeIds and reference-instance
edges; nested closures retain this same static context. The VM's existing demand
table stores each resulting closure. Recursive calls request the existing task,
without runtime template inference, witness liveness optimization, or copying
user data through host/world boundaries. Missing executable instances are explicit
codegen diagnostics; uninstantiated generic runtime entries are rejected.

Concrete generic implementation selections are linked from their existing static
evidence argument maps to instance tasks. Pattern aliases use the selected tag
but the current pattern reference's instantiated type. Neither path guesses a
type from runtime payloads.

Validation: 29 codegen tests, 27 type-pass tests, and 17 static-MIR CLI tests pass.
New runtime tests cover Array(T).type through direct, higher-order and recursive
calls, and generic newtype construction with distinct solved identities. They
drop MIR before execution. Logs: /tmp/mir-instance-emission.log,
/tmp/mir-instance-types-regression.log, /tmp/mir-instance-cli.log and
/tmp/mir-instance-runtime-regressions.log. No full-suite/performance claim.

Remaining gaps include generic local declarations with captured type contexts,
specializing assumed trait/property evidence inside generic bodies, native codec
type consumers, remaining commands, and complete legacy-pipeline removal.

### Static generic instance graph and per-body TypeIds (2026-09-10)

The type pass now retains GenericInstance nodes keyed by resolved declaration
identity and solved type arguments. Each node shares the original HIR and owns
a sorted table of syntax-node TypeIds plus edges to referenced generic instances.
A worklist substitutes already-solved IDs throughout each declaration and follows
transitive generic references before sealing. Ordinary recursive references close
back to the existing instance. Codegen does not perform this substitution.

Implicit generic arguments are publication requirements even when absent from
the result signature. An Unknown argument is diagnosed during the static pass;
seal rejects missing argument outcomes or missing reference-instance edges.
Instance tables and reference edges appear in the stage dump with stable IDs.

This is the static producer side of the next assembly step. Codegen has not yet
switched to consuming these instance bodies; codec, instance-specific trait and
property consumers, and complete old-pipeline removal remain outstanding. The
current static expansion has a 4096-instance resource limit and reports an error
rather than publishing a truncated graph. No performance claim is made.

Validation: all 27 type-pass tests and all 27 current codegen tests pass. Added
coverage checks transitive Int/String instantiations of Array(T).type, recursive
instance identity, and rejection of unfilled phantom arguments. Existing repeated
build/full-dump and inventory-order determinism tests also pass. Logs:
/tmp/mir-instance-closure.log and /tmp/mir-instance-codegen-regression.log.

### Generic reference substitutions survive the type pass (2026-09-10)

The per-reference parameter-to-argument-slot table previously lived only in
Solver and was discarded after resolution. It now belongs to MIR as
type_instances, indexed by HirId, and appears in the staged graph dump. Its
slots undergo the existing final normalization, preserving Known/Unknown/
Conflicted outcomes and rigid outer parameters without repeating signature
matching in codegen. This retains static evidence; passing runtime witnesses
through generic closures/native calls remains the next implementation step.

All 24 type-pass tests pass. The new regression checks independent Int/String
instantiations, forwarding a rigid outer generic parameter, sealing, and equal
slot IDs/full dumps across repeated builds. Log:
/tmp/mir-retained-generic-arguments.log.

### Solved parsers and real serve request diagnostics (2026-09-10)

JSON/YAML/TOML parsing now consumes the solved Value metadata argument and
materializes validated input directly in the current VM heap. Session data
limits apply; parse failures retain the original input handle and provenance
in BlameError. No user value crosses a host/world-copy boundary.

Native closures requiring opaque ABI type captures are linked from the admitted
numeric native module/type slot. Diagnostic snapshots consume solved metadata
IDs and stamp newly allocated diagnostic values without reconstructing types.
The real serve request regression now passes, including a failed first request
whose diagnostics are collected before a successful second request.

Validation: 17 static-MIR CLI tests, 28 codegen tests, the existing serve request
test and two parser handle/data-limit tests pass. The parser/diagnostic codegen
test also covers collecting a parse failure through unwrap exactly once.
Logs: /tmp/mir-parser-{static-cli,codegen,vm,blame-diagnostic}.log and
/tmp/mir-solved-{native-serve,serve-diagnostics}.log. No full-suite or performance
claim is made. Codec encode/decode/schema and generic runtime type/evidence
consumers remain incomplete; remaining consumer migration and complete removal
of the old pipeline are still required. Earlier parser/serve failures below
describe the preceding checkpoint, not the current implementation.

### Real run/serve CLI uses the sealed session and host event loop (2026-09-10)

run_command no longer calls Engine preparation, recovery or run_pending. The
inventory admits the selected application plus a compiler-owned adapter and the
existing run/serve policy into one graph before the three passes. An explicit
generic entry.Run(State)/entry.Serve(State) adapter proves the nominal interface.
Only its exact selected application import receives the compiler-owned graph
edge; ordinary imports retain workspace dependency/private-module restrictions.
Best-effort diagnostics come from the same MIR without a second resolver.

Static callback signatures also supply host protocol TypeIds. execute_run owns
one session through config, host.configure, direct data materialization, the
existing register-based resources_provider, initializer and the event loop.
State, globals, properties and callbacks stay inside the VM. The resources
provider passes prepared/default data by original handles. EES inputs/configs
cross the external I/O boundary as host JSON; they do not transport actor state
or reintroduce host property materialization. Output serialization borrows the
VM string. Host.finish runs on success and failure.

Connecting generic entry families exposed an order-dependent solver bug:
instantiation could copy an inferred Record before its annotation refined it
to a nominal type, losing phantom type arguments. Provisional Record/ArrayLiteral
instances now retain refinement edges and update only when the source constructor
changes. A three-module phantom-generic regression proves the fix without any
entry/builtin name special case.

Validation: 16 static-MIR CLI tests pass, including a real SQLite EES request,
reply and exit with explicit Value input, and serve startup/EOF. The run_ regression
selection has six passes and three failures, all in helpers requiring codec.
The existing serve request test fails explicitly at the pending text parser.
These gaps are not routed through legacy metadata decoding: codec, parsing and
schema generation reject solved-session execution until their witnesses and
BlameError handling are implemented. Therefore command routing is migrated,
but full run/serve language coverage is not yet achieved.

The two generated-adapter tests, 23 type-pass tests, 27 codegen tests and three
VM session/resource-handle tests pass. Logs: /tmp/mir-host-adapter-fixed.log,
/tmp/mir-host-types.log and /tmp/mir-real-{run,serve}-*.log. No performance result
is claimed. Next work is solved parsing/diagnostic collection and codec/evidence
consumers, followed by remaining commands and final legacy-pipeline removal.

### Compiled policy callbacks retain one VM session (2026-09-10)

compile_run now reads the closed policy adapter signature and records the
TypeIds of Env, Caps, Resources, State, Event and Effects. It rejects inconsistent
state identities, non-array effects, malformed callback shapes and remaining
type parameters before a VM exists. The compiled artifact/link carries unary
and binary callback adapters, without a second compilation during execution.

The VM's private SolvedRunSession installs the execution graph and data once,
then retains one WorkWorld, Main TypeImage and QuotaAccount across configuration,
initialization and reducer calls. Capabilities, initializer, reducer and state
are VM-local Val handles. Callback invocation moves the owning Rust container
without relocating its heap. Configuration/initialization cannot be repeated;
a failing callback terminates this session and cannot be retried with fresh fuel.

A test runs the actual run policy through separate VM calls and checks original
actor payload identity across three events, stable TypeImage storage, decreasing
shared fuel and rejection of repeated initialization. Another checks callback
failure and absence of retry/quota reset. These are the internal session mechanics;
the host protocol adapter and CLI run/serve entry point are still not connected,
so non-test builds currently report unused staged session code.

Validation: 27 codegen tests, both VM session tests and cargo check -p telora
pass. Logs: /tmp/mir-run-session-{codegen,tests,cli-check}.log. Diff/source-size
checks pass with large-file review warnings. No performance claim is made.

### Run policy executes in a single graph; JSON borrows VM values (2026-09-10)

The existing std/_entry/run policy now has assembly tests placing it and an
entry.run application in one MIR. A source adapter forms the policy MainType
record, allowing the static pass to prove the interface. The resulting single
compiled session configures sources/capabilities, creates the initializer and
reducer, handles Initialize and emits Output("42") plus Exit(0) after an actor
Reply. Another case exercises a transition without a reply. No Engine, runtime
assignability check or world transfer participates in these tests.

JSON stringify and configured pretty formatters in solved sessions now borrow
the original Value graph, sharing the direct serializer used by CLI eval.
TypeImage records the formatter input TypeId from the admitted native ABI's
solved signature. Runtime serialization checks that ID instead of inferring an
owner from the input or constructing old metadata. The writer takes immutable
heap views and allocates traversal scratch/output only, without unwrapping into
a second VM value graph. Pretty formatting covers nested and empty containers,
including zero-width indentation.

Validation: all 26 codegen tests and 14 static-MIR CLI tests pass; logs are
/tmp/mir-run-policy-{core,cli}.log. Diff/source-size checks pass with existing
large-file review warnings. No performance measurement was made.

The actual run/serve CLI still requires the host adapter and event loop using
one retained VM world. These tests establish the policy path, not CLI migration.
Serve additionally needs solved JSON parsing and diagnostic/codec support.
Remaining generic witnesses, construction checks and final old-pipeline removal
are still open.

### Solved Dyn witnesses and actor state stay inside the VM (2026-09-10)

Solved sessions now implement Dyn pack, project_with, desc and primitive checks
using their closed TypeImage IDs. Pack retains the original metadata Val and
payload Val; projection compares exact TypeIds and returns the original payload.
Nominal wrappers do not pass primitive checks, and distinct nominal types cannot
project to one another. The outer Dyn wrapper does not inherit the payload's
nominal stamp. No descriptor decoding, runtime type interning or host/world
payload copy participates in this path. Other Dyn observations remain explicit
unsupported operations in solved sessions, without a legacy fallback.

The actual std/actor.service and std/entry.run constructors now execute through
this path. A VM test checks exact heap-handle and nominal-TypeId identity before
packing, through an actor reducer, and after projection. The CLI eval-with test
also executes actor.service and returns its updated state. This prepares actor
execution; the run/serve CLI itself still needs assembly onto the new pipeline.

Validation: 23 codegen tests, the dedicated actor handle-identity test and all
14 static-MIR CLI tests pass. Logs: /tmp/mir-solved-dyn-core.log and
/tmp/mir-solved-dyn-cli.log. This checkpoint makes no performance claim and does
not close generic witnesses, remaining metadata/Dyn observations, construction
checks, run/serve/LSP or final legacy-pipeline removal.

### Newtype values and statically selected trait implementations (2026-09-10)

Newtype constructor occurrences now consume their solved nominal TypeId.
MakeNewtype validates the indexed skeleton and allocates the existing one-element
VM container, retaining the payload Val. Nested newtypes keep both the original
payload object and its inner TypeId. Constructor patterns extract that payload
from the statically established shape. Concrete applications of generic newtype
families work; constructing a parameter-dependent nominal type inside generic
code still requires a compiled type witness and is explicitly rejected.

Trait member selection now records the member index and the implementation
SymbolId chosen by the static evidence graph. Codegen emits a demand of that
implementation's session slot followed by the existing field read. Impl method
tables are lazy global tasks, so recursive trait methods reuse the same table
without recursively expanding code or resolving methods in the VM. Ordinary
runtime dependencies include the selected implementation's globals. Concrete
and property-blanket selections work where the method body needs no additional
runtime evidence witness; assumed generic evidence still has an explicit gap.

Array indexing and tuple projection also lower from their solved shape. No
constructor or trait operation calls the old compiler, type store inference or
host property materialization. This retains existing container/field layout;
runtime representation redesign remains separate work.

Twenty-two codegen tests and the previously failing provider alias/factory CLI
regression pass. Coverage includes nested and concrete-generic newtypes, nominal
equality, concrete and blanket trait choices, recursive methods, first-class
method references and global captures. A VM test compares exact nested heap
handles and TypeIds, proving that newtype wrapping does not copy payload data.

Twenty-two type-pass tests and thirteen static-MIR CLI tests also pass. Logs:
`/tmp/mir-newtype-trait-{core,types,cli,static-cli}.log`. Diff/source-size checks
pass with large-file review warnings.

This closes the newtype/trait failure reached by that check regression, not all
generic evidence/codegen cases. Parser recovery diagnostics, run/serve/LSP,
construction checks, generic witnesses, remaining metadata consumers and final
old-pipeline removal/acceptance remain pending. No performance claim is made.

### Ordinary check uses a sealed session initialization root (2026-09-10)

The CLI check command no longer calls Engine.recover_with_resolver. Both modes
use the same inventory and three MIR passes. --only-types returns the static
outcomes without constructing a VM or reading data contents. Ordinary check
requires a successful seal, compiles a distinct Check root, links data and ABI
references, then executes with one VM-owned session state and quota account.

The Check root installs all discovered source-global and property thunks, then
demands their slots. Function definitions create closures but do not call their
bodies. Native/data bindings are available before any demand; dependency reads
reuse the existing lazy table, so shared globals/properties are not copied or
reinitialized at module boundaries. Uncaught VM errors currently stop execution
at the first failing root. Static diagnostics still collect across the graph.

VM.check_linked returns only diagnostics. No initialized property/global value
is exported to a host object or moved to another world. The successful root is
Unit; it does not pretend to be a user export. Check output retains the diagnostic
and summary schema, adds static_seconds/execution_seconds, preserves undeclared
module warnings and excludes builtin modules from the user dependency count.
Execution time includes codegen/link/data preparation and VM initialization.

Regression work also restores explicit-open-import precedence over implicit
prelude defaults while explicit prelude imports participate in ambiguity. This
choice is closed by symbol-resolve, not repeated by the type or execution stages.
Query displays nominal type names instead of internal SymbolId formatting.

New tests exercise unused failing globals, uncalled failing function bodies,
forced property failures, demand cycles, data-before-property order, unread
malformed data under --only-types and suppression of VM execution after type
errors. The existing check regression selection currently has nine passes and
three failures: newtype/trait execution, parser-recovery diagnostic fallout and
an unmigrated run invocation. Those are remaining assembly gaps; connecting
ordinary check is not a claim that all language cases or the full suite pass.

Focused validation: 21 codegen tests, four symbol-pass tests and 13 static-MIR
CLI tests pass (including eval/eval-with). Logs: `/tmp/mir-check-core-tests.log`,
`/tmp/mir-check-symbol-tests.log`, `/tmp/mir-check-static-cli-tests.log`, and
`/tmp/mir-check-regression-tests.log`. Diff and source-size checks pass with
large-file review warnings.

Construction checks, generic metadata/evidence, remaining codegen forms,
run/serve/LSP and removal of the old implementation remain pending. No performance
claim is made at this checkpoint.

### Solved pattern branches and native algebraic values (2026-09-10)

The new emitter now lowers match, guards, if-let, let-else and boolean short
circuiting. Pattern binders and bare constructor names consume the resolver's
distinct Bound outcomes, including imported aliases. Constructor patterns follow
already-bound declaration links to the selected variant; they never evaluate a
constructor to discover its tag. Tuple/record extraction uses the solved shape,
and record pattern field constraints now run in the static type pass.

Option, Result and FoldControl member selection records a variant index in MIR.
MakeVariant consumes that index and the native family identity from TypeImage,
using the existing VM representation. These native layouts are independent of
their generic arguments; constructing them does not require runtime type
inference or a reconstructed descriptor. Generic nominal type witnesses remain
separate unfinished work. Solved nominal enum payload extraction validates the
session TypeId directly instead of consulting the old declared-type metadata map.

Twenty codegen tests pass, including std/option.map through its ordinary generic
signature, imported constructor aliases, native fold-control callbacks, nested
patterns, guards, early returns, short-circuit laziness, escaping pattern captures,
record field errors and property providers reducing previous values. Twenty-two
type-pass tests and six focused CLI eval/eval-with tests pass. CLI property
fixtures now consume query results through match rather than discarding them
before returning a constant. Logs: `/tmp/mir-pattern-{core,types,cli}-tests.log`.

This closes prerequisites for ordinary module initialization; ordinary check is
still on the old Engine and is not claimed migrated. Session-wide check roots,
construction checks, generic metadata/evidence, run/serve and old-pipeline removal
remain pending. No performance claim or full-suite acceptance is made here.

### Required property reads and VM-local lazy results (2026-09-10)

GetTypeProp/GetMemberProp now demand the execution slot selected by solved
owner/property IDs and member position. Codegen installs provider thunks; the VM
alone owns their Pending/Running/Ready/Failed state. Providers reduce in declaration
order, may demand top-level globals, and cache only their final raw property value.
Field/variant contexts use the sealed skeleton's owner, index, name and payload
type IDs. No runtime resolver, inference or old Engine participates in this path.

Required reads cannot return language None: missing presence is invalid bytecode.
The source-level optional query remains explicit: known absence emits None;
known presence emits GetTypeProp followed by Some. First-class/dynamic queries
branch on HasTypeProp/HasMemberProp before issuing the required read. ABI adapters
are selected by admitted native module identity and defining ABI key.

The cached result is a VM Val, not a host PropertyValue or DataWorld. Completion
and repeated reads transfer handles inside the same Work world; they neither
export/reimport nor relocate the property object. Only intermediate reduce inputs
need Some wrappers. Failed reads return Err(Failed(id)), reuse the recorded root
failure, and neither retry the provider nor emit another diagnostic.

Tests cover declaration-order reduction, first-class queries, shared global
dependencies, absent/present reads, actual property/global cycles and field/variant
contexts. A retained-VM test compares exact heap handles across repeated required
reads and checks that failed reads reuse one diagnostic ID and execute once.
Malformed required reads are rejected rather than converted to None.

Validation: six focused CLI eval/eval-with tests pass. Full core tests report
404 passed and 74 failed; the HEAD baseline c7e37b2 reports 400 passed and the
same 74 failing test names. Those existing legacy module/semantic/workspace
failures remain an assembly gap, not a passing full-suite claim. Logs:
`/tmp/property-{baseline,current}-core-tests.log`, `/tmp/property-cli-tests.log`.
Diff and source-size checks pass (existing large-file review warnings remain).

Generic property specialization, evidence adapters, automatic construction checks
and the remaining metadata consumers are still pending. Ordinary check/run/serve
and full removal of the old pipeline remain unfinished. No performance result is
claimed for this checkpoint.

### Concrete TypeId metadata values (2026-09-10)

Concrete T.type now lowers to a solved TypeId constant. The runtime value has a
distinct inline Type classification; it is not an Int or a reconstructed tree of
TypeDescriptor objects. Bytecode linking validates the ID against the installed
session TypeImage. Type aliases preserve identity, and metadata equality compares
the solved IDs. ValueRef.represented_type_id exposes the represented type, separate
from the type of the metadata value itself.

Codegen does not treat a TypeMetadata operand as an executable global dependency.
Reading the skeleton of a decorated or recursive type therefore does not demand
its property providers. Uninstantiated generic metadata still requires explicit
witness lowering and is rejected; no runtime inference substitutes for it. The
legacy world publisher rejects copying the new session-local metadata rather
than remapping it through the old type store. JSON does not serialize metadata.

Fifteen codegen tests and five focused CLI eval tests pass. New coverage checks
primitive/array/recursive nominal metadata, alias identity and a decorated type
whose provider would fail if executed: reading .type still succeeds.

This supplies the runtime identity operand for property queries. GetTypeProp,
provider reduce execution and the remaining metadata consumers are not yet
connected. Full pipeline assembly and performance evaluation remain pending.

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
