# Ontology module check observations

Single sequential ordinary release `check` per selector; no warmups or repeated
sampling. No optimization or comparative performance conclusion is drawn.

- Telora HEAD: `5fdd848`, with local test organization and documentation changes.
- Ontology revision: `1a871a0da47ec677bccf4922d4432ccc2e254d7e`.
- Command: `target/release/telora -C ../lab-ws/lab-ontology/ontology check SELECTOR`.
- Check/static/execution times are JSON summary measurements. Peak RSS comes
  from `/usr/bin/time -v`, converted from KiB to MiB.

| Selector | Exit | Check ms | Static ms | Execution ms | Peak RSS MiB | Unknown | Conflicted |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| @src/intent | 1 | 408.3 | 408.3 | 0 | 43.4 | 17 | 1 |
| @src/ontology | 1 | 380.9 | 380.9 | 0 | 41.2 | 17 | 1 |
| @src/query | 0 | 120.1 | 110.7 | 9.4 | 25.5 | 0 | 0 |
| @test/intent | 1 | 466.2 | 466.2 | 0 | 45.1 | 17 | 1 |
| @test/ontology | 1 | 642.4 | 642.4 | 0 | 55.3 | 17 | 4 |
| @test/diagnostics/rejections | 1 | 410.3 | 410.3 | 0 | 44.6 | 17 | 1 |
| @test/model-rules | 1 | 368.6 | 368.6 | 0 | 42.1 | 17 | 2 |
| @test/test_knowledge | 1 | 386.6 | 386.6 | 0 | 42.6 | 17 | 2 |
| @test/query | 0 | 296.4 | 269.7 | 26.8 | 43.7 | 0 | 0 |

All summaries report zero unproven bounds. Catalog time is 0.61–0.73 ms.
Process wall times in the table's order are 420, 390, 120, 470, 660, 420,
370, 390 and 300 ms (the time tool prints only hundredths of a second).

Only the query selectors complete evaluation. The seven other selectors stop
in the static phase; their times must not be treated as full successful check
costs. They share a diagnostic at `src/ontology.telora:24`:
`cannot unify module std/type-property with Fn(?) -> ?`.
The source imports `std/type-property as property` and then uses
`@property(PropertyTarget.Field)`. This identifies a name-resolution interaction
to investigate, not a proven root cause or an authorization to modify assets.
Additional conflicts involve distinct nominal types in `test/ontology` and
`test/test_knowledge`, and the `@check` contract in `test/model-rules`.
The rejection test also stops on the shared source-module error; its name alone
does not establish that this particular failure is expected.

These broader observations limit architecture acceptance: passing the existing
language suite does not establish compatibility with all ontology modules.
No ontology assets were modified. Raw JSONL, stderr and time output are retained
locally in `/tmp/ontology-module-observations/`, keyed by slash-to-hyphen selector.

## Follow-up isolation (not a performance sample)

In `/tmp/telora-ontology-audit-bItBfg`, a separate copy explicitly imports
`std/prelude as prelude` and qualifies carrier declarations as
`@prelude.property(...)`. The original workspace is unchanged. On that copy,
`check --only-types @test/test_knowledge` reports zero Unknown, but still one
conflict (`cannot unify Order with Customer`). This separates the ordinary
explicit-import shadowing from the remaining type conflict. No reserved-name
exception should be added to the resolver to bypass this shadowing.

A small isolated module reusing a `Fn(Type) -> Fn(Type, Option(Link)) -> Link`
provider on distinct nominal types, including a forward-referenced
`Array(Type)` with heterogeneous metadata, passes. Thus the remaining conflict
has not yet been reduced to a generic property-reuse or metadata-array failure.

## Configured decorator correction

Subsequent reduction isolated the missing condition: the provider is imported
from another module. A `Fn(Type) -> Fn(Type, Option(Link)) -> Link` factory used
with `A.type` and `B.type` incorrectly unified A with B. Configured decorators
now allocate parameter slots and fit argument values to those contracts, matching
ordinary call semantics instead of directly equating actual argument types.
The language `static-property-evidence` case checks both resulting Link values.

With this compiler fix and only the temporary prelude qualification, ordinary
check succeeds for src/ontology, src/intent, test/ontology, test/intent,
test/test_knowledge and test/diagnostics/rejections. These were debug semantic
checks, not performance samples. model-rules retains one Unchecked completion
conflict and zero Unknown. A reduced two-module fixture reproduces that conflict
when a checked record construction with an empty array precedes the Unchecked
completion. The isolated fixture is under /tmp/telora-ontology-audit-bItBfg.

Validation: complete `cargo test --workspace`, the 403-case language runner,
release build and `git diff --check` pass. Logs are /tmp/mir-decorator-workspace.log,
/tmp/mir-decorator-language.log and /tmp/mir-decorator-release.log.

## Unchecked boundary correction

The second reduction confirmed that an earlier construction with an empty array
can supply a provisional record shape before the imported nominal annotation
arrives. Unchecked construction must await its owner's nominal identity, and
Unchecked completion must preserve its directional Fit edge while the expected
type still has that provisional shape. Equating either too early contaminated
the common type slots and produced a misleading @check contract error.

The new unchecked-array-boundary language fixture fails with the preceding
release binary and passes with the fix. It checks ordinary valid construction,
empty-array rejection, invalid candidate rejection and valid candidate completion.
The full language suite now contains 404 cases. Workspace tests also pass.
Ordinary check of model-rules in the temporary copy now succeeds with zero
Unknown, Conflicted and unproven bounds; all seven previously failing selectors
have now completed ordinary check after the isolated source qualification and
the two compiler corrections. Original ontology assets remain unchanged.

Semantic logs: /tmp/mir-unchecked-language.log, /tmp/mir-unchecked-workspace.log,
and /tmp/mir-unchecked-model-rules.jsonl. No new performance conclusion is drawn.

The new release build also passes the isolated workspace's ordinary tests:
model-rules 10/10, query 239/239, intent 24/24 and ontology 129/129. The unchanged
`scripts/test-model-diagnostics.py`, copied beside this temporary workspace and
run against the new release executable, verifies all six intentional rejection
cases (messages, execution phase, rule modules and exact subject spans).
test_knowledge exports support data rather than Test values; `test` correctly
reports no direct Test exports, while its ordinary check succeeds.
Release build log: /tmp/mir-unchecked-release.log. Test JSONL files:
/tmp/mir-unchecked-tests-{model-rules,query,intent,ontology,diagnostics-rejections}.jsonl.
