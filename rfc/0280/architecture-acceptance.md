# Integrated architecture acceptance, 2026-09-11

This records the current validation of the replacement compiler/execution pipeline on
`feat/0280-arena-type-consumers`. It is not acceptance of a comparative performance
claim or of the separately deferred runtime representation optimization.

Final acceptance remains open. The subsequent nine-module ontology observation
found seven static failures. A temporary isolated copy with explicitly qualified
prelude property declarations removes the shared 17 Unknown slots, but
`@test/test_knowledge` initially still reported `cannot unify Order with Customer`.
This was reduced to a cross-module configured decorator equating metadata
arguments instead of fitting its function parameter contracts, and is now fixed
with language regression coverage. Six formerly failing selectors complete
ordinary check in that isolated copy. `model-rules` still exposes a separate
Unchecked completion conflict; it now has a small isolated reproduction involving
an earlier construction with an empty array. The original ontology assets remain
unchanged. Final acceptance is still open pending that boundary investigation.
See [the broader observation](observations/2026-09-11-ontology-modules.md).

## Scope and evidence

The controlling scope is the three-pass, session-owned MIR and the later agreed
Initialize WorkWorld publication boundary. Standalone mode, changing Dict/Struct
representation, field-offset optimization and a new native backend are excluded
by the user. Runtime values are initialized eagerly as roots; dependency reads
within initialization remain demand evaluated. No compatibility resolver is used.

| Requirement | Current implementation and validation |
| --- | --- |
| One module graph, stable inventory IDs, reachable CST/HIR only | `module-resolve.rs::resolve_with_requests` sorts the inventory, allocates ModuleIds before reads and visits reachable modules once through an ordered pending set. Module pass tests cover diamond discovery and inventory permutation. |
| Data modules supply a static Value contract without content access | The module pass synthesizes `decl data: Value` and never invokes the source reader for data. CLI `static_mir_check_injects_data_and_blocks_execution_after_type_errors` checks invalid data still passes only-types and fails ordinary check. |
| Whole-graph symbol closure before typing | `symbol-resolve.rs::resolve` indexes every reachable provider/export before resolving references, then records explicit outcomes. Wildcard imports remain scope entries. DuplicateDefinition and AmbiguousImport retain separate evidence. Symbol tests and CLI open-import tests pass. |
| Prior-stage failures are authoritative | `type-resolve.rs::generate` inherits resolve failures into type slots; it does not retry names. Query integration checks Known, true Unknown and resolve-derived Conflicted together without executing division or parsing invalid data. |
| Syntax slots first, auxiliary generic slots during solving | MIR lowering owns dense HIR/type-slot identity. `type-resolve` fills the one graph through proxy roots, structural terms and constraints. It does not clone module type environments. Required-slot and generic-instance tests cover signatures, bodies, explicit holes, callbacks and partial specialization. |
| Static solving has no VM/heap capability | Production module/symbol/type passes accept syntax/MIR and source inputs, with no VM or runtime heap inputs/dependencies. `static_input::Inventory::solve` invokes those passes directly. CLI and language sentinels verify type errors and data contracts without user execution; LSP tests use the static path. |
| Unknown and Conflicted remain queryable; success is closed | `arena.rs::finalize` retains required Unknown slots; conflict evidence is reported as produced. `mir/seal.rs` validates required slots, instances, layouts, member selections, properties, checks and call contracts. Both check modes now use the same seal gate; query consumes unsealed diagnostic MIR intentionally. |
| Identical full inputs produce stable IDs | The expanded `sealed_full_build_is_independent_of_inventory_enumeration_order` test compares complete MIR dumps, TypeImages, bytecode, native links, execution graphs and result IDs for four inventory orders. It includes recursive nominal types, cross-module generics, two partial-function-value instances and a property depending on a global. Identity across different source revisions is not promised. |
| Codegen consumes solved evidence after solver destruction | `type_resolve::resolve` drops Solver before returning. Codegen's public inputs are SealedMir; it reads resolved SymbolIds, GenericInstanceIds, member selections, checks and final TypeIds. There is no source resolver/type-solver callback in codegen or execution linking. |
| Flat type image and runtime identity | `TypeImage::from_mir` retains arena indices, layouts and generic/nominal definitions. VM heaps receive this image before data or initialization. `heap/copy.rs::validate_session_type` validates the existing ID against the common MainWorld image and copies value representation without rebuilding type identities. |
| Separate property presence and value | Static property records/evidence establish presence and provider contracts. ExecutionGraph maps typed property keys to one ordered provider chain. VM demand tests verify shared property values, failure identity, cycles and source provenance. Dynamic capability validation runs during initialization, not static inference. |
| One Initialize WorkWorld, one publication, fresh entry WorkWorld | `vm/solved-check.rs::initialize_linked_world` executes the initialization root. `freeze_initialized_world` requires all initializers ready. `heap/publish.rs::publish_initialized_roots` uses one forwarding table for all roots, validates before commit, and preserves sharing. The returned WorkWorld uses a fresh heap. `initialization_snapshot_keeps_graph_keys_and_shared_objects_in_main` checks these identities. |
| Failed initialization does not publish a successful result | CLI eval tests reject unused failing globals/properties with empty stdout. Test-session integration verifies initialization cycles abort before any case. Recoverable failures in dispatched test thunks retain the test-result protocol. Host effects are not retroactively rolled back. |
| All commands use the replacement pipeline | `static_cli`, `eval_cli`, `test_cli`, run/serve setup in `main.rs`, and LSP consume Inventory/MIR or linked solved artifacts. CLI integration covers check/query/eval/eval-with/run/serve/test behavior; LSP tests cover non-execution and document invalidation. Replaced Engine/WorkspaceBuilder analysis paths are absent from core module exports and these command routes. |
| Phase diagnostics and paths remain usable | Parser validity is recorded in MIR so recovery does not invent missing-export errors. Unknown language checkers require the originating primary label and zero execution. Workspace/crate discovery shares canonical path identity, including relative `..` paths and unpublished editor file suffixes. Both original ontology command modes succeed. |

## Verification

`cargo test --workspace` passed: 393 library tests, 2 binary unit tests and 66 CLI
integration tests. The latter includes the complete 403-case language acceptance
runner. Doc tests also passed. After mechanically moving codegen tests into
separate files, all 89 codegen tests passed again, including the expanded
determinism fixture. Two moved `include_str!` paths were updated to their new
locations before this successful rerun.

`cargo build --release -p telora` and `git diff --check` passed.
`scripts/check-source-size.sh` passes its hard limits. Its existing soft-review
notices remain for `type-resolve/tests.rs` and `vm/execute.rs`; no size baseline
exception was added. The oversized production-plus-test `codegen.rs` was split
into production code and two included test files without removing assertions.

Logs: `/tmp/mir-final-audit-workspace.log`,
`/tmp/mir-final-audit-codegen.log`, `/tmp/mir-final-audit-release.log`,
`/tmp/mir-final-audit-source-size.log`, and `/tmp/mir-final-audit-targets.log`.

## Performance and deferred work

The user requested a release observation without a new performance conclusion.
[The recorded sample](observations/2026-09-11-release.md) is pinned to `4ee3312`:
281.6 ± 5.2 ms only-types and 325.0 ± 16.2 ms ordinary ontology check. It is not a
measurement of the later seal/path/documentation changes. No cumulative speedup,
comparative allocation gain or broad benchmark acceptance is inferred here.

The architecture replacement is integrated and its existing semantic/CLI suite
passes; broader ontology validation is not complete. More economical initialization allocation, direct field offsets and
broader performance characterization are subsequent work, not fallback paths in
the delivered compiler. The historical incremental RFC checkpoints must not be
read as present implementation or current initialization semantics.
