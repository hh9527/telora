# Rust test migration inventory

This inventory records the completed migration of public language behavior from
Rust unit tests to Telora fixtures. The migration started with 21,062 lines in
`crates/telora-core/src/**/tests/*.rs`. After every Rust test namespace was
reviewed, the replaceable tests were removed together. The retained Rust suite
contains 9,486 lines, a reduction of 11,576 lines (55%).

The language suite contains 197 independently reported cases. Its sources do
not participate in the Rust build, so changing a language expectation no longer
recompiles `telora-core`.

## Replacement evidence

| Fixture surface | Rust behavior replaced |
| --- | --- |
| `check/compiler-semantics` | evaluation, closures, recursion, control flow, patterns, propagation, spreads, casts, operators, and tail calls |
| `check/type-inference` | generic contracts, local generalization, recursive inference, explicit type application, and type families |
| `check/diag-*` | parser, elaboration, type, trait, property, module, and intrinsic diagnostics |
| `check/module-interfaces` | imports, exports, re-exports, namespaces, private nominal identity, and cross-module generic interfaces |
| `check/stdlib-semantics` | Option, Result, Dict, codec, schema, Fmt, and Display contracts |
| `eval/stdlib-collections` | Array, Dict, String, Path, and equality results |
| `eval/codec-schema`, `eval/enum-codec` | structural and recursive codecs, enum representations, JSON schema, and JSON/TOML/YAML string parsing |
| `eval/data-modules` | manifest-backed JSON, TOML, and YAML module loading and typed decoding |
| `eval/runtime-intrinsics`, `eval/diag-*` | dynamic projection, cast, unwrap, diagnostic intrinsic, bounds, and non-finite runtime behavior |
| `eval/display`, `eval/properties` | Display, interpolation, typed properties, member properties, and property-driven blanket implementations |
| `eval/interpreter`, `eval/reflection`, `eval/regex` | interpreter lifting, Dyn projection and observers, indexed reflection, and regex values |
| `eval/type-families` | local, imported, recursive, and composed nominal type families |
| `query/*` | published type schemes, constraints, inferred export types, and canonical trait identities |
| `query-at/recovery` | public recovery facts around damaged source |

Successful `check` fixtures are loaded by one best-effort process. Simple
diagnostic fixtures share another process and are assigned back to their case
by source identity. `eval`, `query`, `query-at`, and diagnostics whose primary
source is a dependency run independently. One generated Telora checker validates
all captured JSON/JSONL observations.

## Retained Rust boundaries

Rust tests remain only where the public command surface cannot prove the
contract. These are implementation invariants, not duplicate language examples.

| Area | Retained contract |
| --- | --- |
| `bytecode`, `lir`, `vm` | register/link validity, call windows, fuel accounting, stack/allocation quotas, traces, and malformed bytecode rejection |
| `heap`, `type_store` | compact values, graph copying, publication atomicity, cycles, canonical and nominal identity, and failed interning rollback |
| `types` | solver/TypeGraph separation, canonical scheme identities, host bindings, partial fact scheduling, tool-stage accounts, and bytecode witnesses |
| `module` | stable slots, persistent World roots, closure publication, session isolation, module quotas, recovery fact identity, exact provenance, and callback continuations |
| `module_id`, `workspace` | filesystem containment, vendor selection, catalog visibility, stable logical identity, overlays, cancellation, and atomic publication |
| `parser`, `syntax` | lossless CST reconstruction, chunk bridges, damaged-tree recovery, source ranges, and unknown token preservation |
| `source`, `document`, `semantic`, `query` | byte/character positions, atomic edits, partial semantic indexes, completion state, revision and cancellation identity |
| `evaluation` | partial dependency scheduling, failure propagation, cancellation, and deterministic budget truncation |
| `json`, `toml`, `yaml` | validation-before-materialization, structural limits, exact ranges, provenance, and format boundary cases |
| `hir`, `pattern`, `elaboration` | hygienic lowering and internal binding/type fact indexing |
| `sha256`, `regex` | algorithm vectors and native numeric-plan boundaries |

`bounded_generic_calls_forward_hidden_trait_evidence` remains as a Rust test
because the equivalent public package currently encounters an internal trait
implementation binding while loading. It must move only after that public path
is valid evidence.
