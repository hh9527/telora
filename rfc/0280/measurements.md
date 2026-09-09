# RFC 0280: Historical Measurements

This appendix preserves the evidence behind the session-wide redesign in
[the RFC](../0280-demand-driven-inference-materialization.md). It records the
initial baseline and the implementation through `293098c`, before the session
architecture was implemented. References below to phases and outstanding work
belong to the earlier consumer-migration plan; the main RFC defines the current
plan and acceptance gates. No speedup of the new design has been measured.

## Initial baseline

Release with debug information, rustc 1.98.1; no source instrumentation.
Hardware counters were unavailable. CPU observations use `cpu-clock:u` at
499 Hz with DWARF stacks. Wall measurements use one warmup and five runs,
without a profiler or a concurrent build. Allocation counts use heaptrack.

The synthetic modules contain N independent three-field structs and N simple
functions with explicit Fn(T) -> Int contracts. Both variants declare the same
Label property/provider; only the property variant applies it to all N structs.
All checks succeed. Measurements include process and builtin startup.

| N | Plain, mean +/- standard deviation | With property |
| --- | ---: | ---: |
| 100 | 262.9 +/- 13.6 ms | 278.0 +/- 15.7 ms |
| 200 | 365.6 +/- 6.2 ms | 420.4 +/- 25.0 ms |
| 400 | 617.4 +/- 15.1 ms | 716.1 +/- 25.9 ms |

At N=400, plain/property checks allocated 2,777,025 / 3,104,739 times.
Allocation stacks through normalize accounted for 27.95% / 27.82%; stacks
through tool expression inference accounted for 37.78% / 39.82%.
CPU samples through normalize accounted for 11.14% / 10.79%, and through
tool expression inference for 27.60% / 28.21% (1,221 / 1,400 samples).
These are overlapping inclusive paths, not additive phase budgets.
Allocation counts are not cumulative bytes or live-object counts.

The repository codec-schema check also succeeded in ten sampled runs:
normalize accounted for 11.62% and tool inference for 17.88% of 671 CPU
samples. Its builtin startup share was much larger than in the synthetic cases.
Some stack roots were unavailable and inline expansion was disabled in perf
reports; these percentages are approximate observations, not speedup promises.

Raw local observations and inputs are in `/tmp/telora-perf-173/README.md`.
They are temporary artifacts, not a prerequisite for future acceptance. The
repository's `scripts/measure-tool-inference.py` reproduces the synthetic cases.
This was the initial baseline investigation; subsequent instrumentation and
checkpoint measurements are recorded below.

## Implementation checkpoints through 293098c

### Arena queries and first compatibility reductions

The branch now has a descriptor-free slot/row head view, graph predicates for
unresolved variables and metadata-returning function chains, an Unchecked head
guard, and direct Struct/Enum result construction. Tests cover late binding,
known-slot replacement, conflict propagation, 16,384-deep graphs without
descriptor views, and equivalence with the prior normalized predicates.
Function call inspection has subsequently been changed to retain parameter/result
slot edges, including the call-entry openness needed by generic constraints.
Closure expectations now retain those edges as well. Full language validation
exposed a missed boundary: normalizing a callback signature after an earlier
argument solved T structurally detached its result from the shared T slot.
Keeping the callback's parameter/result edges restores subsequent nominal
refinement. The existing nominal-equality behavior suite verifies this with
`choose_with_factory([{value: 42}], fn() { item })`; all 22 cases pass after
the fix. A structural solution is not proof that a slot can be detached.

The query/adapter checkpoint passed the workspace suite: 326 core tests, 41 CLI
tests and all 400 language groups. The subsequent call-edge change passed all
326 core tests; the later tool-graph checkpoint below includes broader validation
and the closure-edge fix.
Default and counter-enabled release builds succeeded before that call-edge change.
The `inference-profile` feature and `TELORA_INFERENCE_PROFILE=1` emit per-solver
JSON counters; default builds contain neither their fields nor increments.

Before the call-edge change, five-sample release comparisons at sizes 100/400
showed no demonstrated wall-time gain: changes ranged from about -1% to +2.3%
across constant, typed/property types, shared-wide and shared-deep cases. Do not
claim a speedup from those noise-sized differences. For the original property-400
input, allocations decreased from 3,104,739 to 3,045,669 (about 1.9%), while
peak heap remained 37.89 MB. Raw measurements are in
`/tmp/rfc0280-stage1-comparison.jsonl` and `/tmp/rfc0280-stage1-heap.txt`.

Counter-enabled property-400 preparation reported 184,289 normalization roots,
657,346 normalized nodes, 69,623 descriptor views, 6,539 body cache hits,
2,148 empty entries, 1,708 revision-stale entries and 11,714 unindexed body
requests. These are post-query-migration counts, not before/after counter deltas.
They reinforce continuing with call/evidence graph migration, not declaring
the RFC complete after local clone reductions. Tools and the broader template,
nominal identity and consumer migrations remain outstanding.

### Owned tool evidence (in progress)

Tool expression records and runtime evidence now publish into one owned TypeGraph
with AnalysisTypeId roots and a shared publication session. Function arity and
nominal-owner selection inspect graph heads without rebuilding signatures or
unrelated records. Inferred records still override supplied expression facts.
The solver can be destroyed before evidence consumption; tests explicitly cover
this lifetime and parameter/result/root sharing.

This is an intermediate migration, not completion of phase 3. Roots rejected by
final-type publication retain an explicit compatibility descriptor, preserving
open-function arities and existing error behavior. Selected owner substitution
and runtime metadata construction still materialize descriptors. The direct
graph-to-metadata bridge, open-record snapshot representation, and measurements
of subsequent migrations remain outstanding. The measurements below show that
this checkpoint does not meet the performance acceptance gate.

### Tool graph checkpoint measurements — 2026-09-09

The checkpoint includes the call/closure edge changes and owned tool evidence.
It passed `cargo test --workspace` (328 core tests, 41 CLI tests, all 400 language
groups, and the remaining workspace/doc tests), default release build,
`git diff --check`, and source-size checks. The three existing source-size
advisories remain. It is not ready to merge on performance grounds.

Preserved uninstrumented binaries use the same optimized release profile with
debug information: baseline `13d5859`, query-only `telora-stage1`, and current
`telora-tool-graph`, all under `/tmp/telora-perf-173/`. The initial sweep used one
warmup and five samples at N=100/200/400, including a constant control and shared
wide/deep cases. Builds and tests were finished before timings; profilers ran
separately. Most current medians regressed by 2–5% against baseline. A second
sweep reversed binary order and used ten samples at N=400:

| Workload | Baseline median | Current median | Change |
| --- | ---: | ---: | ---: |
| Constant control | 138.16 ms | 140.19 ms | +1.47% |
| Plain typed types, 400 | 610.37 ms | 619.88 ms | +1.56% |
| Property types, 400 | 695.76 ms | 714.67 ms | +2.72% |
| Shared wide, 400 | 410.76 ms | 427.03 ms | +3.96% |
| Shared deep, 400 | 372.92 ms | 382.15 ms | +2.48% |

The repository codec-schema check also succeeded in all runs. Its ten-sample
median increased from 172.69 to 178.41 ms (+3.31%). Mean/stdev were
172.53/1.44 and 181.39/10.97 ms; the current run had an outlier, so the mean
ratio is not a precise estimate of its regression.

Heaptrack used the original plain/property-400 inputs, identical to the earlier
baseline profiles. RSS is the median of five separate uninstrumented checks,
not heaptrack RSS. Heap MB below are decimal; RSS is KiB.

| Metric | Plain baseline → current | Property baseline → current |
| --- | ---: | ---: |
| Allocation calls | 2,777,025 → 2,785,722 (+0.31%) | 3,104,739 → 3,128,479 (+0.76%) |
| Peak heap | 35.81 → 36.13 MB | 37.89 → 38.20 MB |
| Peak RSS median | 49,236 → 49,460 KiB | 51,188 → 51,644 KiB |

Property allocation stacks through normalize decreased from 863,876 to 647,551
(-25.04%), but stacks through tool inference increased from 1,236,348 to
1,263,668. Current publication stacks account for 109,332 calls; stacks matching
TypeGraph descriptor conversion account for 54,796. These categories overlap
and are not all incremental costs. Compared with the query-only checkpoint's
3,045,669 allocations, current property allocations increased by 2.72%.
There is no demonstrated total allocation or peak-memory benefit.

A software CPU profile of five current property checks collected 1,441 samples.
Inclusive normalize/tool-inference shares were approximately 9.09%/30.19%,
versus the earlier baseline's 10.79%/28.21%; tool evidence publication accounted
for about 3.05%. Sampling uncertainty and overlapping call paths prevent adding
these shares or treating them as an exact explanation of wall-time changes.
The evidence supports fewer normalization allocations, but the intermediate
graph publication plus retained tree adapters has not delivered an overall win.

Raw local artifacts: `/tmp/rfc0280-tool-graph-comparison.jsonl` (three versions,
all scales), `/tmp/rfc0280-tool-graph-reverse.jsonl` (reversed ten-sample sweep),
`/tmp/rfc0280-tool-graph-codec.json`, `/tmp/rfc0280-tool-graph-rss.txt`,
`/tmp/rfc0280-tool-graph-{plain,property}-heap.txt`, and
`/tmp/rfc0280-tool-graph-perf-flat.txt`. Current heaptrack/perf files and the
preserved binaries remain local profiling artifacts, not repository assets.
## Session source reuse checkpoint, 2026-09-09

The uninstrumented optimized binary `/tmp/telora-perf-173/telora-session-sources`
adds shared discovery/loader parse records to `293098c`. It does **not** include
the subsequent lazy recovery change. Each ordering used one warmup and five
samples per case; the table combines both orderings (ten samples per version).
No builds, tests or profilers ran alongside these timings.

| Case | 293098c median ms | Source reuse median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 157.71 | 158.69 | +0.63% |
| typed-100 | 268.62 | 267.35 | -0.47% |
| property-100 | 291.11 | 297.52 | +2.20% |
| shared-wide-100 | 226.50 | 231.07 | +2.02% |
| fanout-100 | 274.53 | 275.46 | +0.34% |
| diamond-100 | 287.90 | 280.31 | -2.64% |
| typed-400 | 653.47 | 622.62 | -4.72% |
| property-400 | 752.47 | 720.61 | -4.23% |
| shared-wide-400 | 453.86 | 448.05 | -1.28% |
| fanout-400 | 658.83 | 655.63 | -0.48% |
| diamond-400 | 693.52 | 682.28 | -1.62% |

Fanout has N small imported modules. Diamond has N arms sharing one nominal
definition/value module through re-exports. The benchmark checks each root, not
each dependency separately. Small controls do not establish a general speedup;
the regressions need reassessment after the next reduction in duplicate work.

For property-400, allocation calls decreased from 3,128,479 to 3,071,136
(-1.83%), peak heap from 38.20 to 37.20 MB (-2.62%). Uninstrumented peak RSS
medians over five runs were 51,676 and 50,376 KiB (-2.51%). Heaptrack ran
separately from timing/RSS measurements. This is one workload, not proof of
bounded retained memory for all module graphs or repeated sessions.

Evidence: `/tmp/rfc0280-session-sources-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-session-sources-property-heap.txt`, and raw
`/tmp/telora-perf-173/session-sources-property.heap.zst`. Source reuse passed
the full workspace suite (330 core, 41 CLI tests) and release build. The two
new tests check captured source identity after disk changes and failed overlay
retention. HIR/type/interface reuse and execution-free inference are not delivered
by this checkpoint.

## Demand-driven recovery checkpoint, 2026-09-09

`/tmp/telora-perf-173/telora-lazy-recovery` additionally skips partial analysis
when strict Analysis exists. Two orderings, each with one warmup and five samples,
compare it with the source-reuse binary. Results below are pooled medians; no
build/test/profiler ran concurrently. These are incremental gains, not gains
against the original pre-arena baseline.

| Case | Source reuse median ms | Lazy recovery median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 156.29 | 156.26 | -0.02% |
| typed-100 | 263.50 | 235.59 | -10.59% |
| property-100 | 287.53 | 258.23 | -10.19% |
| shared-wide-100 | 223.86 | 220.49 | -1.51% |
| fanout-100 | 271.55 | 253.27 | -6.73% |
| diamond-100 | 280.59 | 259.55 | -7.50% |
| typed-400 | 627.56 | 496.62 | -20.86% |
| property-400 | 717.03 | 596.08 | -16.87% |
| shared-wide-400 | 440.03 | 449.24 | +2.09% |
| fanout-400 | 649.58 | 574.03 | -11.63% |
| diamond-400 | 680.95 | 588.47 | -13.58% |

Property-400 allocation calls decreased from 3,071,136 to 2,558,532 (-16.69%);
peak heap from 37.20 to 35.16 MB (-5.48%). A separate five-run RSS comparison
gave medians of 50,320 and 47,780 KiB (-5.05%). The property fixture is identical
to the prior heap runs. Broader memory/phase attribution remains outstanding.

All workspace tests (330 core, 41 CLI including language acceptance), release
build and diff/source-size checks passed. Existing recovery tests exercise type
errors, syntax errors, independent facts, module cycles, runtime failures and
rule/data provenance. This change still retries analysis after strict failure;
it is not the final shared-solver recovery path or the zero-execution static API.

Artifacts: `/tmp/rfc0280-lazy-recovery-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-lazy-recovery-property-heap.txt`,
`/tmp/telora-perf-173/lazy-recovery-property.heap.zst`,
`/tmp/rfc0280-lazy-recovery-workspace.log` and
`/tmp/rfc0280-lazy-recovery-release.log`. A separate fifteen-sample follow-up
(`/tmp/rfc0280-lazy-recovery-controls.jsonl`, lazy version first) measured
shared-wide medians 440.98 -> 418.40 ms and constant 142.05 -> 141.17 ms.
The earlier shared-wide regression did not repeat. Its old-version mean/stdev
were 437.43/14.89 ms versus 418.08/3.78 ms; constant timings also shifted
between batches. Do not treat this follow-up as a precise 5% shared-wide gain
or compare absolute timings from different batches as equivalent conditions.

## Import reference graph checkpoint, 2026-09-09

Compared preserved uninstrumented optimized binaries `telora-lazy-recovery`
(`b0aa34d`) and `telora-import-graph` in `/tmp/telora-perf-173`. Each ordering
used one warmup plus five measured samples; table medians pool both orderings.
Builds, tests and profilers were not concurrent with timings.

| Case | b0aa34d median ms | Import graph median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 157.54 | 156.11 | -0.91% |
| fanout-100 | 251.07 | 247.05 | -1.60% |
| diamond-100 | 256.02 | 255.30 | -0.28% |
| fanout-400 | 564.04 | 556.79 | -1.29% |
| diamond-400 | 579.04 | 573.14 | -1.02% |

The changes are small, and do not establish a substantial speedup. Separately,
diamond-400 allocation calls decreased 2,722,078 -> 2,703,271 (-0.69%), peak
heap increased 27.72 -> 27.83 MB (+0.40%). Five uninstrumented RSS runs gave
medians 42,188 -> 42,572 KiB (+0.91%). New graph records and retained module
targets coexist with the old skeleton/interface structures; this intermediate
ownership cost is not hidden by the reduction in temporary allocations.

The runner now supports `--save-workspace` to a new directory for separate
profiling of the exact generated source. Evidence:
`/tmp/rfc0280-import-graph-{comparison,reverse}.jsonl`, saved workspace
`/tmp/rfc0280-import-graph-workspace`,
`/tmp/rfc0280-{lazy-recovery,import-graph}-diamond-heap.txt` and raw
`/tmp/telora-perf-173/{lazy-recovery,import-graph}-diamond.heap.zst`.
Validation logs: `/tmp/rfc0280-import-graph-{workspace,release}.log`.
All workspace tests passed (333 core, 41 CLI including language acceptance).
This covers only the import-reference foundation, not full information-graph
solving, static export/constructor resolution or zero-execution type inference.

## Shared artifact consumers checkpoint, 2026-09-09

Preserved optimized uninstrumented binaries: `/tmp/telora-perf-173/telora-import-graph`
(`a7fda2b`) and `/tmp/telora-perf-173/telora-shared-artifacts`. Two orderings each
used one warmup and five samples, without concurrent tests/builds/profilers.
Pooled medians:

| Case | a7fda2b median ms | Shared artifacts median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 146.36 | 144.76 | -1.10% |
| property-400 | 581.86 | 576.86 | -0.86% |
| shared-wide-400 | 430.24 | 427.52 | -0.63% |
| diamond-400 | 562.68 | 561.71 | -0.17% |

These small timing changes do not establish a clear speedup. Property-400
allocation calls decreased 2,558,518 -> 2,541,510 (-0.66%), peak heap decreased
35.16 -> 33.83 MB (-3.78%). Five uninstrumented RSS runs gave medians
48,036 -> 46,376 KiB (-3.46%). Memory profiling ran separately from timing.
These `check` workloads use recovery loading and measure HIR sharing; they do
not quantify the separate removal of strict-loader skeleton reconstruction.

All workspace tests passed (333 core, 41 CLI including language acceptance),
as did release build and diff/source-size checks. Evidence:
`/tmp/rfc0280-shared-artifacts-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-{import-graph,shared-artifacts}-property-heap.txt`, raw
`/tmp/telora-perf-173/{import-graph,shared-artifacts}-property.heap.zst`, and
`/tmp/rfc0280-shared-artifacts-{workspace,release}.log`.
This checkpoint shares HIR within each analysis, not yet across the full
session or strict-failure recovery. Static type-contract elaboration remains
the next major execution boundary to replace.

## Direct declaration contracts checkpoint, 2026-09-09

Compared preserved optimized uninstrumented `telora-shared-artifacts` (`b5464c4`)
and `telora-static-contract` binaries in `/tmp/telora-perf-173`. One warmup and
five measurements per version/workload in each of two opposite version orders;
table medians pool ten samples. No builds/tests/profilers ran alongside timings.
These are end-to-end CLI `check` costs, including initialization, not isolated
inference phase durations.

| Case | b5464c4 median ms | Static contracts median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 147.30 | 135.79 | -7.81% |
| typed-400 | 489.25 | 403.07 | -17.61% |
| property-400 | 579.26 | 504.96 | -12.83% |
| shared-wide-400 | 429.03 | 259.92 | -39.42% |
| shared-deep-400 | 380.26 | 243.68 | -35.92% |
| diamond-400 | 562.09 | 553.76 | -1.48% |

Separate memory measurements:

| Workload | Allocation calls before -> after | Peak heap MB before -> after | Five-run RSS median KiB before -> after |
| --- | --- | --- | --- |
| property-400 | 2,541,510 -> 2,300,182 (-9.50%) | 33.83 -> 32.46 (-4.05%) | 46,376 -> 45,856 (-1.12%) |
| shared-wide-400 | 2,254,205 -> 1,432,801 (-36.44%) | 19.71 -> 18.82 (-4.52%) | 34,824 -> 34,308 (-1.48%) |

The property baseline heap measurement is the preserved shared-artifacts result
above; shared-wide heaps and both RSS comparisons were collected in this batch.
Heaptrack RSS/runtime includes profiler overhead and is not used for the table.
These results target declaration contracts; they neither cover all type syntax
nor establish zero execution for the whole type phase. Do not add percentages
from different checkpoints or extrapolate them directly to ontology.

Initial CLI serve failures exposed shared structural recursion through a nominal
boundary. That conversion was fixed, with adjacent coverage rejecting anonymous
structural cycles. A later complete nominal body now refines an earlier stub
without replacing its ID. Final validation passed 338 core and 41 CLI tests
(including language acceptance), release build and diff/source-size checks.

Artifacts: `/tmp/rfc0280-static-contract-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-contract-memory-workspace`,
`/tmp/rfc0280-static-contract-{property,wide}-heap.txt`,
`/tmp/rfc0280-shared-artifacts-wide-heap.txt`, raw
`/tmp/telora-perf-173/static-contract-{property,wide}.heap.zst` and
`/tmp/telora-perf-173/shared-artifacts-wide.heap.zst`.
Final validation logs: `/tmp/rfc0280-static-contract-refined-{workspace,release}.log`.

## Symbolic family contract applications, 2026-09-09

Baseline is the qualified concrete-contract extension preserved as
`/tmp/telora-perf-173/telora-qualified-contract`; candidate is the symbolic family
application checkpoint. Both are optimized release builds with debug symbols and
without inference profiling. Each workload/version has ten samples pooled from
two opposite version orders, each with one warmup and five measurements. Builds,
tests and profilers were terminal before timing began. Results are end-to-end
CLI `check` times, not isolated type-inference durations.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 135.67 | 131.88 | -2.79% |
| family-contracts-400 | 287.04 | 188.88 | -34.20% |
| qualified-family-contracts-400 | 300.08 | 191.12 | -36.31% |
| typed-types-400 | 406.83 | 397.44 | -2.31% |
| property-types-400 | 499.11 | 495.60 | -0.70% |
| shared-wide-400 | 254.03 | 251.42 | -1.03% |
| module-diamond-400 | 546.97 | 551.71 | +0.87% |

The new family workloads declare one `Box(T)` with `value: T` and `items:
Array(T)` fields, then 400 identity functions with `Fn(Box(Int)) -> Box(Int)`
contracts. The qualified case imports the family from a dependency namespace.
They target contract applications; repeated family use inside type declaration
bodies still uses the previous evaluator pipeline. Small movements in control
workloads do not establish a general improvement, especially the slightly slower
module-diamond case.

Separate heaptrack runs of qualified-family-contracts-400 measured allocation
calls decreasing 1,471,737 -> 1,082,587 (-26.44%) and peak heap decreasing
15.65 -> 12.77 MB (-18.40%). Profiler runtime and RSS are not timing or resident
memory baselines. No ontology performance claim follows from these synthetic
results.

Validation passed the full workspace suite (343 core, 41 CLI including language
acceptance, other workspace/doc tests), release build and diff/source-size
checks. Evidence: `/tmp/rfc0280-static-family-{workspace,release}.log`,
`/tmp/rfc0280-static-family-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-family-memory-workspace`,
`/tmp/rfc0280-{qualified-contract-family,static-family}-heap.txt`, and raw
`/tmp/telora-perf-173/{qualified-contract-family,static-family}.heap.zst`.

## Static declaration bodies with source-use origins, 2026-09-09

Baseline: `4eb1b22`, preserved as `/tmp/telora-perf-173/telora-static-family`.
Candidate: `/tmp/telora-perf-173/telora-static-bodies-reused`, including source
origin projection and reuse of referenced metadata. Both optimized release
binaries retain debug symbols and have no inference profiling feature enabled.
One warmup plus five samples in each of two opposite version orders yields ten
samples per workload/version. No builds, tests or profilers ran during timings.
These are end-to-end CLI `check` costs, including builtin initialization.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 130.43 | 113.86 | -12.70% |
| types-400 | 262.94 | 167.86 | -36.16% |
| repeated-family-400 | 215.97 | 141.99 | -34.26% |
| family-contracts-400 | 187.04 | 171.21 | -8.47% |
| typed-types-400 | 399.38 | 269.91 | -32.42% |
| property-types-400 | 491.33 | 361.97 | -26.33% |
| shared-wide-400 | 254.89 | 236.63 | -7.16% |
| module-diamond-400 | 549.18 | 533.95 | -2.77% |

Separate property-types-400 heaptrack runs measured 2,294,702 -> 1,771,413
allocation calls (-22.80%) and 32.61 -> 31.73 MB peak heap (-2.70%). No claim
about process RSS follows from profiler RSS. The first source-aware implementation
rebuilt referenced metadata and reached 34.16 MB peak heap even though allocation
calls fell. Reusing the existing objects at source reference edges, while retaining
each occurrence's location, reduced this intermediate peak to 31.73 MB. Intermediate
timings and heaps are retained separately and are not used in the table above.

Validation passed 349 core tests, 41 CLI tests including language acceptance,
all remaining workspace/doc tests, release build and diff/source-size checks.
Coverage includes the original codec rule-location regression, distinct locations
for shared field types, alias and family origins, nested substituted argument
origins, metadata reuse without mutating source locations, generic shadowing,
zero execution fuel for supported static declarations, and actual tool/property
execution quota enforcement.

These are incremental synthetic-workload results. Recursive definition components,
bounded/unresolved forms, property/construction preparation and failure recovery
still include execution; the full session graph is not yet execution-free.

Final evidence: `/tmp/rfc0280-static-bodies-reused-{workspace,release}.log`,
`/tmp/rfc0280-static-bodies-reused-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-bodies-memory-workspace`,
`/tmp/rfc0280-{static-family,static-bodies-reused}-property-heap.txt`, and raw
`/tmp/telora-perf-173/{static-family,static-bodies-reused}-property.heap.zst`.
Intermediate evidence: `/tmp/rfc0280-static-bodies-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-bodies-property-heap.txt` and
`/tmp/telora-perf-173/telora-static-bodies-before-reuse`.

## Static trait and property constraint facts, 2026-09-09

Compared `7bd53b8` (`telora-static-bodies-reused`) with `telora-static-constraints`
in `/tmp/telora-perf-173`. Both optimized release binaries retain debug symbols
without inference profiling. One warmup and five samples per version/workload
in each of two opposite version orders yield ten pooled samples. No builds,
tests or profilers overlapped timings. Results are end-to-end CLI `check` costs.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 114.09 | 113.44 | -0.57% |
| property-constraints-400 | 187.42 | 146.89 | -21.62% |
| qualified-property-constraints-400 | 192.78 | 146.73 | -23.89% |
| typed-types-400 | 268.99 | 264.49 | -1.67% |
| property-types-400 | 364.37 | 365.56 | +0.33% |
| module-diamond-400 | 535.03 | 535.64 | +0.11% |

The new workloads declare a Label type and 400 generic identity functions with
`for(T: Property(Label)) Fn(T) -> T` contracts. The qualified version imports the
Label declaration from a namespace. They establish signatures without calling
the functions or running providers. Control workloads show no general speedup
from this step. Do not add these percentages to earlier checkpoint percentages
or apply them to ontology.

Separate qualified-property-constraints-400 heaptrack runs measured 807,115 ->
728,656 allocation calls (-9.72%). Peak heap rounded to 12.06 MB in both runs;
there is no observed peak-memory benefit at that reporting precision. Profiler
RSS/runtime are not uninstrumented performance measurements.

Final workspace validation passed 351 core and 41 CLI tests, including language
acceptance, and all other workspace/doc tests. Release build and diff/source-size
checks passed. An initially invalid new family test fixture was corrected to a
valid constrained type alias; production code did not change for that correction.
Tests cover zero execution fuel for mixed trait/property bounds and a constrained
family signature, plus preservation of duplicate-property diagnostics. Existing
acceptance covers missing property evidence and provider publication errors.

Evidence: `/tmp/rfc0280-static-constraints-final-workspace.log`,
`/tmp/rfc0280-static-constraints-release.log`,
`/tmp/rfc0280-static-constraints-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-constraints-memory-workspace`,
`/tmp/rfc0280-{static-bodies-constraints,static-constraints}-heap.txt`, and raw
`/tmp/telora-perf-173/{static-bodies-constraints,static-constraints}.heap.zst`.

## Deferred construction dependency preparation, 2026-09-09

Baseline `c45c55d` is `/tmp/telora-perf-173/telora-static-constraints`; candidate
is `/tmp/telora-perf-173/telora-deferred-construction`. Optimized release binaries
retain debug symbols without inference profiling. Two opposite version orders,
each with one warmup and five samples, yield ten pooled samples per case/version.
No builds, tests or profilers overlapped timings. These are end-to-end CLI `check`
times, not isolated inference measurements.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 114.34 | 114.51 | +0.15% |
| types-400 | 170.00 | 167.36 | -1.55% |
| checked-types-400 | 479.42 | 268.62 | -43.97% |
| repeated-family-400 | 139.95 | 139.31 | -0.45% |
| typed-types-400 | 263.08 | 264.44 | +0.52% |
| property-types-400 | 360.96 | 358.57 | -0.66% |
| module-diamond-400 | 530.21 | 528.03 | -0.41% |

The new checked-types workload declares 400 nominal Int wrappers, each with an
inline `@check` that returns Ok(()) for positive values and Err(blame!(...))
otherwise. No wrapper values are constructed. This measures checker preparation
and registration rather than executing checks on user values. Static declaration
elaboration no longer repeatedly prepares all construction dependencies while
later types are still pending. Existing registration and execution checks remain.
Control movements do not establish a general pipeline improvement; these results
must not be extrapolated to ontology or added to earlier checkpoint percentages.

Separate checked-types-400 heaptrack runs measured allocation calls decreasing
3,560,823 -> 1,288,887 (-63.80%) and peak heap decreasing 24.04 -> 23.55 MB
(-2.04%). The much larger allocation reduction indicates transient work removal;
profiler RSS and runtime are not uninstrumented measurements.

Full workspace validation passed 352 core and 41 CLI tests (including language
acceptance) and all remaining workspace/doc tests. Release and diff/source-size
checks passed. A new zero-fuel regression verifies static duplicate declarations
are reported before running checker value dependencies. Existing construction,
recursive-check and codec acceptance tests continue to pass.

Evidence: `/tmp/rfc0280-deferred-construction-{workspace,release}.log`,
`/tmp/rfc0280-deferred-construction-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-deferred-construction-memory-workspace`,
`/tmp/rfc0280-{static-constraints,deferred-construction}-checked-heap.txt`, and
raw `/tmp/telora-perf-173/{static-constraints,deferred-construction}-checked.heap.zst`.
