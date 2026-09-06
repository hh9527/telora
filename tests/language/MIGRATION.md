# Rust test migration inventory

This inventory records the completed migration of public language behavior from
Rust unit tests to Telora fixtures. The migration started with 21,062 lines in
`crates/telora-core/src/**/tests/*.rs`. After every Rust test namespace was
reviewed, the replaceable tests were removed together. The retained Rust suite
contains 9,486 lines, a reduction of 11,576 lines (55%).

At the end of that migration, the language suite contained 197 independently
reported fixture entrances. Its sources do
not participate in the Rust build, so changing a language expectation no longer
recompiles `telora-core`.

## Replacement evidence

The paths below record the original Rust-to-Telora migration. The subsequent
deferred Test migration maps them to their current locations in the next section.

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

In that layout, successful `check` fixtures were loaded by one best-effort process. Simple
diagnostic fixtures share another process and are assigned back to their case
by source identity. `eval`, `query`, `query-at`, and diagnostics whose primary
source is a dependency run independently. One generated Telora checker validates
all captured JSON/JSONL observations.

## Deferred Test migration

[Issue #152](https://github.com/hh9527/telora/issues/152) tracks the completed
RFC 0267 migration. Runtime semantic fixtures now execute named Test exports,
with actual calculations and explicit assertions inside thunks. No Rust test
was removed in this migration.

| Previous fixture | Current fixture | Test cases |
| --- | --- | ---: |
| `check/compiler-semantics` | `test/compiler-semantics` | 38 |
| `check/stdlib-semantics` | `test/stdlib-semantics` | 5 |
| `check/type-inference` | `test/type-inference` | 16 |
| `check/module-interfaces` | `test/module-interfaces` | 15 |
| `eval/codec-schema` | `test/codec-schema` | 1 |
| `eval/enum-codec` | `test/enum-codec` | 1 |
| `eval/data-modules` | `test/data-modules` | 3 |
| `eval/display` | `test/display` | 1 |
| `eval/properties` | `test/properties` | 1 |
| `eval/interpreter` | `test/interpreter` | 1 |
| `eval/reflection` | `test/reflection` | 1 |
| `eval/regex` | `test/regex` | 1 |
| `eval/runtime-intrinsics` | `test/runtime-intrinsics` | 1 |
| `eval/stdlib-collections` | `test/stdlib-collections` | 4 |
| `eval/type-families` | `test/type-families` | 1 |
| 11 `eval/diag-*` fixtures and `check/diag-non-exhaustive-runtime` | `test/runtime-failures` | 12 |

These 16 modules contain 102 independently reported Test cases. The runtime
failure module retains the original messages for bounds, non-finite arithmetic,
dynamic projection, unwrap, must_ok, reflection indices, regex syntax, string
margin/indent, format fragments, and dynamic match failure. Successful semantic
cases retain their original conditions; numeric export IDs now have descriptive
names. Related assertions remain grouped by topic.

The suite now has 199 fixture entrances: 165 `check`, 1 `eval`, 5 `query`,
1 `query-at`, and 27 `test`. The decrease from 210 entrances at migration start
comes from consolidating failure fixtures, not removing assertions. Entrance
counts differ from Test case counts, since one testee can export many Tests.

The migration also fixed three exposed gaps:

- `data-modules` and `runtime-intrinsics` formerly returned booleans without
  asserting them. They now fail on false; TOML expected tools explicitly have
  type `Array(Tool)` for nominal equality.
- Workspace loading now carries and publishes imported trait/property evidence
  roots, as required by the display/property tests.
- Closure free-variable analysis now recognizes predeclared recursive function
  defs, as exercised by the compiler and inference tests inside thunks.

### Retained command boundaries

| Retained surface | Reason |
| --- | --- |
| `check/diag-*`, `check/type-mismatch` | Syntax, static types, imports, module initialization, and diagnostic protocol need their original command boundary. |
| `check/diag-non-tail-depth` | Call-depth exhaustion is terminal and must not count as a successful `should_fail` case. |
| `check/diag-non-finite-float` | The non-finite literal is rejected before a thunk can run. |
| `check/deferred-lazy` | Verifies that `check` neither executes Tests nor reads fixture inputs. |
| `eval/interpolation` | Checks serialized output values, not merely a boolean success flag. |
| `query/*`, `query-at/recovery` | Verify published semantic facts and recovery output. |
| Existing `test/*` protocol fixtures | Verify runner failures, initialization, discovery, fixture expansion, and v2 reports through their checkers. |
| `src/eval/data-modules/*` and helper modules under `src/check/module-interfaces`, `src/eval/properties` | Remain production source imports to preserve static data loading and cross-module interface coverage. |
| `crates/telora/tests/fixtures/performance/*` | Performance workloads are not assertion fixtures. |

Module-level types, decorators, generic functions, and typed family values are
retained where they exercise declaration and interface semantics. Runtime
assertions execute in thunks. Static data imports are not replaced with
`with_fixtures`, which tests a different input boundary.

Validation used debug builds, the full Rust workspace suite (354 tests,
including the language harness), and all 199 language entrances. No release
binary was built. The harness remains grouped by topic; no runtime speedup is
claimed from the migration.

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
for the standalone compiler's scalar interpolation fallback, which does not
load `std/fmt.Display` evidence. `test/trait-evidence` separately covers the
public module path with real Display evidence: direct and explicit generic
calls, generic forwarding, returned closures, imports, and reexports. Its
interpolation stays inside impl methods so ordinary definitions cannot mask
missing runtime dependencies (issue #153).
