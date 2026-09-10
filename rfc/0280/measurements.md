# RFC 0280: Historical Measurements

This appendix preserves the evidence behind the session-wide redesign in
[the RFC](../0280-demand-driven-inference-materialization.md). It records the
initial baseline and subsequent implementation checkpoints. Early references to
phases belong to the earlier consumer-migration plan; the main RFC defines the
current plan and acceptance gates. Later sections record measured incremental
and cumulative improvements; the session architecture remains incomplete.

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
is `/tmp/telora-perf-173/telora-deferred-construction`. Both are optimized release
builds without inference profiling. A later ELF audit found baseline DWARF debug
sections but only the symbol table in the candidate (default release settings).
These results therefore are not a comparison with identical debug-info settings;
the artifact distinction was omitted in the original checkpoint report.
Two opposite version orders,
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

## Static recursive concrete definitions, 2026-09-09

Baseline `d99f2d5` is `/tmp/telora-perf-173/telora-deferred-construction`; candidate
is `/tmp/telora-perf-173/telora-static-recursion`. Both use default optimized release
settings, retain symbol tables but no DWARF debug-info sections, and have no
inference profiling enabled. Two opposite version orders each use one warmup and
five samples per workload/version; medians pool ten samples. No builds, tests or
profilers overlapped timing. Measurements are end-to-end CLI `check` costs.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 112.96 | 112.33 | -0.55% |
| types-400 | 167.38 | 165.98 | -0.84% |
| checked-types-400 | 272.71 | 268.54 | -1.53% |
| recursive-types-400 | 306.65 | 201.03 | -34.44% |
| property-types-400 | 362.33 | 358.39 | -1.09% |
| module-diamond-400 | 526.94 | 526.23 | -0.13% |

The new workload defines 400 distinct `type T = struct {children: Array(T)}`
self-recursive types and exports an integer. It does not construct recursive
values, benchmark recursive generic families, or measure a large mutually
recursive component. Control movements do not establish a general improvement;
do not extrapolate this synthetic result to ontology.

Separate recursive-types-400 heaptrack runs measured 1,530,212 -> 1,104,914
allocation calls (-27.79%) and 23.02 -> 22.23 MB peak heap (-3.43%). Instrumented
runtime/RSS are not ordinary process measurements.

Workspace validation passed (353 core, 41 CLI including language acceptance, and
other workspace/doc tests). The final small adjustment retained the exact legacy
evaluation/validation order for unsupported bodies; all 353 core tests were rerun
on that final code, and its release build passed. Source-size/diff checks passed.
Coverage includes zero-fuel self/mutual recursion and existing recursive metadata,
cross-module values, codec/schema and construction quota behavior.

Evidence: `/tmp/rfc0280-static-recursion-workspace.log`,
`/tmp/rfc0280-static-recursion-final-{core,release}.log`,
`/tmp/rfc0280-static-recursion-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-static-recursion-memory-workspace`,
`/tmp/rfc0280-{deferred-construction-recursive,static-recursion}-heap.txt`, and
raw `/tmp/telora-perf-173/{deferred-construction-recursive,static-recursion}.heap.zst`.

## Recursive consumers reuse solved graph, 2026-09-09

Baseline `f590556` (`telora-static-recursion`) versus `telora-recursive-consumers`,
both in `/tmp/telora-perf-173`, with default optimized release settings and no
inference profiling. Two opposite version orders, one warmup plus five samples
each, yield ten pooled samples per workload/version. No builds/tests/profilers
overlapped timing. Results are end-to-end CLI `check` medians.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 114.61 | 113.85 | -0.66% |
| types-400 | 166.55 | 166.91 | +0.22% |
| checked-types-400 | 271.33 | 270.62 | -0.26% |
| recursive-types-400 | 199.74 | 199.47 | -0.13% |
| property-types-400 | 361.55 | 364.21 | +0.73% |
| module-diamond-400 | 527.93 | 529.57 | +0.31% |

There is no demonstrated timing improvement. This checkpoint removes two metadata
decode paths for supported recursive declarations: initializer-shape validation
uses the solved body ID, and descriptor/signature publication uses the solved
nominal owner ID in the original graph. Unsupported paths retain decoding.
Separate heaptrack runs of recursive-types-400 in the same saved workspace measured
allocation calls 1,104,861 -> 1,091,125 (-1.24%); peak heap remained 22.23 MB at
the reported precision. Profiler runtime/RSS are not ordinary measurements.

Full workspace validation passed (353 core, 41 CLI including language acceptance,
all remaining workspace/doc tests), as did release build and source-size/diff
checks. Existing recursion, identity, codec and provenance regressions pass.
This is consumer migration progress, not evidence of general speedup or completion
of the execution-free session graph.

Evidence: `/tmp/rfc0280-recursive-consumers-{core,workspace,release}.log`,
`/tmp/rfc0280-recursive-consumers-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-recursive-consumers-memory-workspace`,
`/tmp/rfc0280-recursive-consumers-heap.txt`,
`/tmp/rfc0280-recursive-consumers-baseline-heap.txt`, and raw
`/tmp/telora-perf-173/recursive-consumers{,-baseline}.heap.zst`.

## Static self-recursive families, 2026-09-09

This is an incremental comparison against the immediately preceding checkpoint
`d28f6f8`, not the original RFC baseline. Baseline binary is
`/tmp/telora-perf-173/telora-recursive-consumers`; candidate is
`/tmp/telora-perf-173/telora-recursive-family`. Both use default optimized release
settings without inference profiling. Two opposite version orders each use one
warmup and five samples, yielding ten pooled samples per workload/version.
No builds, tests or profilers overlapped timing. Results are end-to-end CLI `check`.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 113.06 | 112.83 | -0.21% |
| family-contracts-400 | 169.16 | 167.96 | -0.71% |
| recursive-types-400 | 200.36 | 201.58 | +0.61% |
| recursive-families-400 | 389.14 | 296.08 | -23.91% |
| property-types-400 | 359.16 | 358.07 | -0.30% |
| module-diamond-400 | 530.17 | 526.86 | -0.63% |

The new workload declares 400 distinct self-recursive `Tree(T)` families with a
value field and an Array(Tree(T)) children field, and one Int application per
family. It constructs no recursive values and has no trait/property bounds.
Control movements do not establish a general improvement; these incremental
percentages cannot be added to earlier checkpoints or extrapolated to ontology.

Separate recursive-families-400 heaptrack runs measured allocation calls decreasing
1,787,362 -> 1,578,545 (-11.68%) and peak heap decreasing 43.56 -> 42.66 MB
(-2.07%). Profiler runtime/RSS are not uninstrumented measurements.

Final full workspace validation passed 355 core, 41 CLI including language
acceptance, and all remaining workspace/doc tests. Release build and source-size/
diff checks passed. Zero-fuel regression coverage includes recursive template
definitions, Int/String applications and phantom identity; changed/reordered
self arguments remain rejected. An initial language acceptance failure in
checked-recursive-types exposed duplicate symbolic metadata roots, fixed by
reusing the reserved self root. Its original five cases and the final full suite
pass without changing acceptance expectations.

Evidence: `/tmp/rfc0280-recursive-family-final-{workspace,release}.log`,
`/tmp/rfc0280-recursive-family-regressions.log`,
`/tmp/rfc0280-recursive-family-checked-regression.jsonl`,
`/tmp/rfc0280-recursive-family-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-recursive-family-memory-workspace`,
`/tmp/rfc0280-recursive-family{,-baseline}-heap.txt`, and raw
`/tmp/telora-perf-173/recursive-family{,-baseline}.heap.zst`.

## Cumulative baseline comparison through 2c426a7, 2026-09-09

Directly reran the original `13d5859` binary against `2c426a7`, rather than
combining checkpoint percentages. Binaries are
`/tmp/telora-perf-173/telora-baseline-13d5859` and `telora-recursive-family` in
the same directory. Both are optimized release builds without profiling features;
the baseline includes DWARF debug information and the candidate uses default
release settings. This debug-info difference remains a comparison limitation.
Two opposite version orders, one warmup and five samples each, yield ten pooled
samples per workload/version. No builds, tests or profilers overlapped timings.

| Case | Initial median ms | Current median ms | Time reduction |
| --- | ---: | ---: | ---: |
| constant | 148.15 | 113.68 | 23.26% |
| typed-types-400 | 615.69 | 267.37 | 56.57% |
| property-types-400 | 705.14 | 360.66 | 48.85% |
| recursive-families-400 | 679.97 | 299.55 | 55.95% |
| shared-wide-400 | 416.21 | 235.23 | 43.48% |
| module-diamond-400 | 666.47 | 531.11 | 20.31% |

These are end-to-end CLI `check` times on identical generated sources, not an
ontology benchmark. Type-heavy cases run approximately 1.77–2.30 times as fast.
Separate property-types-400 heaptrack runs measured allocation calls
3,116,634 -> 1,767,406 (-43.29%) and peak heap 38.04 -> 31.73 MB (-16.59%).
Profiler runtime/RSS are not uninstrumented process measurements.

Evidence: `/tmp/rfc0280-cumulative-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-cumulative-memory-workspace`,
`/tmp/rfc0280-cumulative-{baseline,current}-property-heap.txt`, and raw
`/tmp/telora-perf-173/cumulative-{baseline,current}-property.heap.zst`.

## Constrained family shapes with final obligations, 2026-09-09

Incremental comparison against `2c426a7`, not the original RFC baseline:
`/tmp/telora-perf-173/telora-recursive-family` versus `telora-family-obligations`
in the same directory. Both use default optimized release settings without
inference profiling. Two opposite version orders, one warmup plus five samples
each, yield ten pooled samples per workload/version. No builds, tests or profilers
overlapped timing. Results are end-to-end CLI `check` medians.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 114.38 | 114.53 | +0.14% |
| family-obligations-400 | 238.64 | 170.64 | -28.49% |
| qualified-family-obligations-400 | 249.94 | 178.41 | -28.62% |
| typed-types-400 | 267.18 | 267.98 | +0.30% |
| property-types-400 | 360.92 | 363.57 | +0.73% |
| module-diamond-400 | 533.36 | 530.14 | -0.60% |

New workloads define Label and `Box(T: Property(Label)) = Array(T)`, then 400
identity signatures quantified by the same property bound and taking/returning
Box(T). The qualified version imports Label/Box from a dependency namespace.
No provider or application value is executed. Templates provide structure while
original schemes retain obligations, checked with lexical evidence in final
inference. The implementation also rejects previously accepted invalid constrained
applications in signatures; this correctness change is included in the comparison.
Control movements do not establish a general pipeline gain. Do not extrapolate
these results to ontology or add incremental percentages.

Separate qualified-family-obligations-400 heaptrack runs measured allocation calls
1,132,180 -> 924,536 (-18.34%). Peak heap increased 16.81 -> 17.26 MB (+2.68%):
this checkpoint does not improve peak memory for that workload. Profiler runtime
and RSS are not uninstrumented measurements.

Full workspace validation passed 358 core, 41 CLI including language acceptance,
and all remaining workspace/doc tests; release build and diff/source-size checks
passed. Regressions cover missing Property/trait evidence, zero-fuel applications
with lexical evidence, and the existing type/metadata boundary. An isolated CLI
workspace also confirms a qualified `model.Box(Int)` signature reports missing
Property(Label) evidence at the application location and exits with failure.

Evidence: `/tmp/rfc0280-family-obligations-{core,workspace,release}.log`,
`/tmp/rfc0280-family-obligations-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-family-obligations-memory-workspace`,
`/tmp/rfc0280-family-obligations{,-baseline}-heap.txt`, raw
`/tmp/telora-perf-173/family-obligations{,-baseline}.heap.zst`, and
`/tmp/rfc0280-qualified-obligation.vFmBa0` (isolated negative CLI input).

## Module-owned syntax facts and minimal semantic handoff, 2026-09-09

Incremental comparison against `a6f9950`: `/tmp/telora-perf-173/telora-family-obligations`
versus `telora-module-facts` in the same directory, both default optimized release
without inference profiling. Two opposite version orders, one warmup plus five
samples each, yield ten pooled samples per workload/version. No builds, tests or
profilers overlapped timing. Results are end-to-end CLI `check` medians.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 115.27 | 113.03 | -1.94% |
| typed-types-400 | 268.71 | 269.95 | +0.46% |
| property-types-400 | 365.73 | 357.24 | -2.32% |
| shared-wide-400 | 235.04 | 234.75 | -0.12% |
| module-diamond-400 | 535.41 | 526.45 | -1.67% |

Timing movements are small and do not demonstrate a broad speedup. Prepared syntax
for discovered modules now lives on the ModuleId-indexed row; undiscovered entry
paths retain separate compatibility storage. Semantic snapshot inputs carry only
the required result location rather than a cloned Program. Recovery borrows the
original Program. Per-module Arc still bridges recursive loader borrowing and is
explicitly transitional, not the final session-arena ownership model.

Separate property-types-400 heaptrack runs measured allocation calls
1,767,969 -> 1,751,611 (-0.93%) and peak heap 31.73 -> 29.23 MB (-7.88%).
Profiler runtime/RSS are not uninstrumented process measurements.

Full workspace passed 358 core, 41 CLI including language acceptance, and all
other workspace/doc tests. Release and diff/source-size checks passed. Existing
session source-reuse coverage now checks that discovered syntax resides on its
ModuleId row and survives backing-file changes without reconstruction.

Evidence: `/tmp/rfc0280-module-facts-{workspace,release}.log`,
`/tmp/rfc0280-module-facts-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-module-facts-memory-workspace`,
`/tmp/rfc0280-module-facts{,-baseline}-heap.txt`, raw
`/tmp/telora-perf-173/module-facts{,-baseline}.heap.zst`.

## Session-owned syntax without per-module Arc, 2026-09-09

Incremental comparison against `1f7a52f`: `/tmp/telora-perf-173/telora-module-facts`
versus `telora-owned-syntax` in the same directory. Both use default optimized
release without inference profiling. Two opposite version orders, one warmup and
five samples each, yield ten pooled samples per case/version. No builds, tests or
profilers overlapped timing. These are end-to-end CLI `check` medians.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 115.59 | 114.92 | -0.58% |
| typed-types-400 | 266.22 | 266.95 | +0.27% |
| property-types-400 | 362.49 | 362.03 | -0.13% |
| shared-wide-400 | 232.12 | 234.81 | +1.16% |
| module-diamond-400 | 525.72 | 526.38 | +0.13% |

This checkpoint demonstrates no clear speedup. PreparedModule is owned directly
by the module row; ModuleGraph and ModuleSkeleton no longer implement Clone.
Dependency preparation retains import operands and a cursor across recursive
loading; compilation and recovery borrow session syntax afterward. It removes the
per-module ownership adapter, not AST/HIR's remaining internal trees, HIR Arc,
legacy dependency execution or descriptor consumers.

Separate property-types-400 heaptrack runs measured allocations
1,751,589 -> 1,751,563 (26 fewer), and peak heap 29.23 -> 29.24 MB at the profiler's
display precision. This is not a meaningful memory reduction; inline module rows
also reserve space for absent prepared input. Profiler runtime and RSS are not
uninstrumented measurements. Do not extrapolate this checkpoint to ontology.

Full workspace passed 358 core, 41 CLI including language acceptance, and all
remaining workspace/doc tests. Release and diff/source-size checks passed.
Source-reuse coverage verifies original ModuleId/SourceId and successful loading
after root and dependency source files change.

Evidence: `/tmp/rfc0280-owned-syntax-{check,workspace,release}.log`,
`/tmp/rfc0280-owned-syntax-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-owned-syntax-memory-workspace`,
`/tmp/rfc0280-owned-syntax{,-baseline}-heap.txt`, and raw
`/tmp/telora-perf-173/owned-syntax{,-baseline}.heap.zst`.

## Borrowed HIR and semantic projection inputs, 2026-09-09

Incremental comparison against `64d4e68`: `/tmp/telora-perf-173/telora-owned-syntax`
versus `telora-borrowed-facts` in the same directory. Both use default optimized
release without inference profiling. Each command was measured in two opposite
version orders, one warmup plus five samples each, yielding ten pooled samples
per case/version. No builds, tests or profilers overlapped timing.

HIR ownership is direct, with ToolInferenceContext borrowing it until final
Analysis/PartialAnalysis receives it by move. Semantic projection sorts borrowed
input references rather than cloning complete HIR/type facts at loader/run/eval/test
handoffs. Ordinary check already consumed inputs, and is the control.

End-to-end CLI `test` medians (one trivial successful test importing each workload):

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 115.22 | 115.19 | -0.03% |
| property-types-400 | 372.81 | 369.19 | -0.97% |
| module-diamond-400 | 544.70 | 525.94 | -3.44% |

End-to-end CLI `check` control medians:

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 113.95 | 114.46 | +0.44% |
| property-types-400 | 361.77 | 362.35 | +0.16% |
| module-diamond-400 | 527.84 | 523.80 | -0.77% |

The test diamond workload shows a modest timing improvement; property timing is
small and the constant is unchanged. Check controls show no clear speedup. These
are complete commands, not isolated inference or snapshot timings. The test
wrapper loads the same generated source graph and executes one trivial success;
it does not measure 400 individual test executions. No ontology measurement or
new cumulative-baseline comparison is included.

Separate heaptrack measurements on the test wrappers:

| Case | Allocations before -> after | Change | Peak heap MB before -> after | Change |
| --- | ---: | ---: | ---: | ---: |
| property-types-400 | 1,806,737 -> 1,785,691 | -1.16% | 29.26 -> 29.26 | unchanged at displayed precision |
| module-diamond-400 | 2,614,028 -> 2,535,551 | -3.00% | 32.99 -> 25.97 | -21.28% |

Removing a full input copy lowers diamond peak heap by 7.02 MB; the property
workload's overall peak is unchanged. Profiler runtime and RSS are not
uninstrumented measurements.

Final full workspace passed 358 core, 41 CLI including language acceptance, and all
remaining workspace/doc tests; release and diff/source-size checks passed.
Existing tests cover recovery facts, imported/reexported binders, recursive type
identity, runtime blame sources, test graph composition and run/eval behavior.

The final snapshot still remaps and owns projected type/definition records.
The compiled-module to semantic-input Analysis copy and per-analysis HIR
resolution remain; this is not yet a unified session HIR arena or global ID-only
downstream pipeline, and legacy type-phase execution remains.

Evidence: `/tmp/rfc0280-borrowed-facts-{check,workspace,release}.log`,
`/tmp/rfc0280-borrowed-facts-{test,check}-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-borrowed-facts-memory-workspace`,
`/tmp/rfc0280-borrowed-facts-{baseline-diamond,diamond,baseline-property,property}-heap.txt`,
raw `/tmp/telora-perf-173/borrowed-facts-{baseline-diamond,diamond,baseline-property,property}.heap.zst`.

## Linear projection and shared named roots, 2026-09-09

Incremental comparison against `d5dbf63`: `/tmp/telora-perf-173/telora-borrowed-facts`
versus `telora-linear-projection` in the same directory. Both default optimized
release without inference profiling. Each command used two opposite version
orders, one warmup plus five samples each, for ten pooled samples per case/version.
No builds, tests or profilers overlapped timing.

End-to-end CLI `check` medians:

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 115.29 | 115.02 | -0.24% |
| recursive-families-400 | 296.64 | 296.87 | +0.08% |
| property-types-400 | 366.97 | 360.25 | -1.83% |
| module-diamond-400 | 535.22 | 530.39 | -0.90% |

End-to-end CLI `test` medians (one trivial successful test importing the source graph):

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| property-types-400 | 365.45 | 366.17 | +0.20% |
| module-diamond-400 | 523.12 | 522.00 | -0.21% |

Timing movements are small; no clear general speedup is established. In separate
heaptrack measurements, check property400 allocations were 1,751,533 -> 1,750,653
(-0.05%), peak heap 29.24 -> 29.25 MB. Test diamond400 allocations were
2,534,908 -> 2,535,516 (+0.02%), peak heap 25.98 -> 25.98 MB at displayed precision.
There is no meaningful peak-heap reduction. Profiler runtime/RSS are not
uninstrumented measurements. No new ontology or cumulative baseline measurement.

Projection scans source arrays into contiguous output spans and computes each
child ID using the span base, replacing recursive traversal and per-type remapping
arrays. A regression exposed an existing name-publication copy of solved nominal
nodes; name publication now shares solved nominal IDs and retains necessary
forward placeholders as Ref edges. Temporary name reservations use a Vec of IDs.
This combines projection changes and named-root reuse, not an isolated measurement
of either change.

Workspace validation passed 360 core, 41 CLI including language acceptance, and all
remaining workspace/doc tests. After the final temporary-map-to-vector adjustment,
all 360 core tests and release were rerun and passed. Diff/source-size checks passed.
New regressions verify recursive and shared edges in independent output spans,
consistent named/signature roots, and forward names resolving to shared primitive
roots. The recursive regression initially failed because name publication cloned
the source nominal node; fixing that source duplication made it pass unchanged.

Snapshot-local numerical type order now follows the source arena, rather than DFS.
Every consumer uses the same arithmetic translation, including names, result types
and expression/definition facts. This is not yet direct consumption of one global
session arena: output records are still projected, proxy rows may remain, and
descriptor adapters still exist. Current check execution behavior is preserved.
Type-only check is recorded separately as a follow-up candidate after the original
plan, per user direction.

Evidence: `/tmp/rfc0280-linear-projection-{check,test,workspace,final-core,final-release}.log`,
`/tmp/rfc0280-linear-projection-{check,test}-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-linear-projection{,-test}-memory-workspace`,
`/tmp/rfc0280-linear-projection-{baseline-property,property,baseline-diamond,diamond}-heap.txt`,
raw `/tmp/telora-perf-173/linear-projection-{baseline-property,property,baseline-diamond,diamond}.heap.zst`.

## Direct graph metadata for tool type arguments, 2026-09-09

Incremental comparison against `5320e42`: `/tmp/telora-perf-173/telora-linear-projection`
versus `telora-graph-metadata` in the same directory. Both default optimized release
without inference profiling. Two opposite version orders, one warmup plus five
samples each, yield ten pooled samples per case/version. No builds, tests or
profilers overlapped timing. These are end-to-end CLI check medians.

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 108.11 | 107.54 | -0.53% |
| recursive-families-400 | 294.15 | 289.86 | -1.46% |
| property-types-400 | 362.66 | 362.46 | -0.06% |
| tool-type-arguments-400 | 616.28 | 604.20 | -1.96% |

The new workload declares 400 distinct type-property providers. Each provider
calls a generic identity function on a twelve-element Int tuple before producing
its Label. This exercises tool runtime type-argument metadata, including repeated
references to a shared element type. It does not isolate metadata construction
from the rest of analysis/property execution. Control timing movements are small;
the targeted 1.96% timing reduction does not establish a general speedup.

Separate heaptrack runs on tool-type-arguments-400 measured allocations
2,287,680 -> 2,270,523 (-0.75%) and peak heap 49.55 -> 46.00 MB (-7.16%, 3.55 MB).
A backtrace filter for type_graph_value confirms the benchmark enters the new
builder through evaluate_prepared_tool_expression. Profiler runtime/RSS are not
uninstrumented process measurements. No ontology or new cumulative baseline
measurement is included.

The no-origin runtime type binding path consumes graph nodes directly, reusing
generated values by node ID rather than first expanding a descriptor tree.
Nominal owners are reserved before body traversal; sealed nominal metadata is
reused as before. Crossing a nominal boundary permits structural revisits; pure
structural cycles are rejected. Bound arity scans graph nodes and phantom identity
arguments. Compatibility roots still use descriptors.

Full workspace passed 362 core, 41 CLI including language acceptance, and all
remaining workspace/doc tests; release and diff/source-size checks passed.
New regressions compare descriptor/direct metadata with symbolic phantom arguments,
exercise structural roots crossing nominal cycles and reject a structural self-cycle.
Existing provenance, recursive metadata and property/check fixtures remain passing.

This is one migrated consumer. Origin-bearing construction, owner-evidence generic
substitution and nominal identity argument adapters remain. Construction scratch
is per root; it is not a persistent stage cache or a completed global metadata
arena. CLI check still performs its existing evaluation.

Evidence: `/tmp/rfc0280-graph-metadata-{check,tests,workspace,release}.log`,
`/tmp/rfc0280-graph-metadata-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-graph-metadata-memory-workspace`,
`/tmp/rfc0280-graph-metadata{,-baseline}-heap.txt`,
`/tmp/rfc0280-graph-metadata-builder-heap.txt`, raw
`/tmp/telora-perf-173/graph-metadata{,-baseline}.heap.zst`.

## Batched graph metadata roots, 2026-09-09

Incremental comparison against `27835c3`: `/tmp/telora-perf-173/telora-graph-metadata`
versus `telora-metadata-batch`. Both default release builds; two opposite version
orders, one warmup and five samples per order (ten pooled samples). No builds,
tests or profilers overlapped timing. End-to-end CLI check medians:

| Case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 108.10 | 108.45 | +0.33% |
| property-types-400 | 358.76 | 358.31 | -0.12% |
| tool-type-arguments-400 | 595.74 | 598.19 | +0.41% |
| tool-shared-arguments-400 | 1235.90 | 1228.66 | -0.59% |

The new shared-arguments case executes eight generic identity calls on the same
twelve-Int tuple shape in each of 400 distinct property providers. It tests shared
roots within tool expressions; the existing single-call case is unchanged.
These timing movements do not demonstrate a material speedup.

Separate heaptrack runs on the shared-arguments case measured allocation calls
4,206,159 -> 4,162,278 (-1.04%) and peak heap 147.74 -> 146.39 MB (-0.91%).
Batching therefore removes some repeated allocation, but is not a large memory
improvement either. Profiler time and RSS are not normal runtime measurements.
No new ontology or cumulative baseline measurement is included.

Full workspace passed 363 core, 41 CLI and all remaining tests; release,
diff and source-size checks passed. Regression coverage verifies repeated-root
identity, mixed root ordering, empty input and discarded scratch after failure.
The operation-local table is shared across roots, not retained as a stage cache.
Property evaluation and current check execution behavior are unchanged.

Evidence: `/tmp/rfc0280-metadata-batch-{check,tests,workspace,release}.log`,
`/tmp/rfc0280-metadata-batch-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-metadata-batch-memory-workspace`,
`/tmp/rfc0280-metadata-batch{,-baseline}-heap.txt`, raw
`/tmp/telora-perf-173/metadata-batch{,-baseline}.heap.zst`.

## Session-owned strict analysis, 2026-09-09

Incremental comparison against `7d3be57`: `/tmp/telora-perf-173/telora-metadata-batch`
versus `telora-owned-analysis`. Both default release builds. Two opposite version
orders, one warmup and five measured samples per order, ten pooled samples.
Builds, tests and profilers did not overlap timing.

The runner now supports eval: a generated source wrapper imports the workload,
exports Value.Int(42), and evaluates that export. Every sample checks both exit
status and decoded output. Unlike test/check's WorkspaceBuilder path, eval uses
the modified strict ModuleLoader. A heaptrack filter for compile_telora confirms
that this workload reaches the modified path.

| Eval case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 116.85 | 116.39 | -0.39% |
| property-types-400 | 377.54 | 373.27 | -1.13% |
| module-diamond-400 | 542.41 | 525.09 | -3.19% |

Separate heaptrack runs:

| Eval case | Allocation calls before → after | Peak heap before → after |
| --- | ---: | ---: |
| property-types-400 | 1,812,484 → 1,792,859 (-1.08%) | 29.35 → 29.33 MB |
| module-diamond-400 | 2,616,927 → 2,545,384 (-2.73%) | 25.80 → 26.13 MB |

This removes allocation work but does not demonstrate a peak-heap improvement.
The small timing improvements apply to these strict-loading cases, not universally.
No new cumulative or ontology measurement is included. Profiler time/RSS are not
uninstrumented process metrics.

An initial test-mode comparison did not exercise the modified loader and is kept
as control evidence: constant 117.15 → 118.03 ms (+0.76%), property 372.98 → 373.93 ms
(+0.26%), diamond 534.40 → 533.67 ms (-0.14%). Test/property allocations were effectively
unchanged (1,784,765 → 1,784,826), peak heap 29.27 MB for both. These results prompted
the call-path audit and addition of eval rather than supporting a speedup claim.

Full workspace passed 363 core, 41 CLI including language acceptance, and all
remaining tests; release, diff/source-size checks passed. Existing eval/entry,
dependency identity/provenance and session-source tests cover the modified handoff.
The benchmark wrapper also verifies successful output after analysis ownership
transfers into LoadedModule.

Evidence: `/tmp/rfc0280-owned-analysis-{check,workspace,release}.log`,
`/tmp/rfc0280-owned-analysis-{test,eval}-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-owned-analysis-eval-memory-workspace`,
`/tmp/rfc0280-owned-analysis-eval{,-baseline,-diamond,-diamond-baseline}-heap.txt`,
`/tmp/rfc0280-owned-analysis-eval-path.txt`, raw
`/tmp/telora-perf-173/owned-analysis-eval{,-baseline,-diamond,-diamond-baseline}.heap.zst`.

## Borrowed compiler evidence across closures, 2026-09-09

Incremental comparison against `1b04e09`: `/tmp/telora-perf-173/telora-owned-analysis`
versus `telora-compiler-facts`, both default release. Two opposite version orders,
one warmup and five measured samples per order; medians of ten pooled samples.
Builds, tests and profilers did not overlap timing.

| Command / case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| eval / constant | 108.40 | 109.01 | +0.56% |
| eval / property-types-100 | 168.47 | 162.92 | -3.30% |
| eval / property-types-400 | 362.58 | 345.66 | -4.67% |
| check / constant | 107.95 | 112.80 | +4.49% |
| check / property-types-400 | 358.01 | 341.51 | -4.61% |

The initial check/constant control had candidate samples spanning 105.42–122.31 ms.
A separate follow-up used opposite orders and ten samples per order (twenty per
version): 107.28 -> 106.41 ms (-0.80%). It did not reproduce the initial control
regression. Both runs are retained; no broad startup speedup is claimed. Follow-up
artifacts: `/tmp/rfc0280-compiler-facts-constant-followup{,-reverse}.jsonl`.

Separate heaptrack runs on eval/property-types-400 measured allocation calls
1,783,344 -> 1,619,626 (-9.18%); peak heap stayed 29.17 MB. Filtering the nested
compiler stack confirms the baseline's 160,400 string allocations from cloning
the complete owner-evidence table are absent in the candidate. Local parameter
and capture allocations remain. This supports removal of repeated solved-fact
copying, not elimination of every compiler allocation or a peak-memory reduction.

Full workspace passed 363 core, 41 CLI including language acceptance and all
remaining tests; release, diff/source-size checks passed. Coverage includes generic
hidden evidence, constructors, recursive metadata, lexical scope and diagnostics.
No new cumulative or ontology benchmark is included; profiler runtime/RSS are not
uninstrumented process metrics.

Evidence: `/tmp/rfc0280-compiler-facts-{check,workspace,release}.log`,
`/tmp/rfc0280-compiler-facts-{check,eval}-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-compiler-facts-memory-workspace`,
`/tmp/rfc0280-compiler-facts{,-baseline}-{heap,path}.txt`, raw
`/tmp/telora-perf-173/compiler-facts{,-baseline}.heap.zst`.

## Range-indexed owner capture facts, 2026-09-09

Incremental comparison against `ecb5008`: `/tmp/telora-perf-173/telora-compiler-facts`
versus `telora-owner-index`, both default release. Two opposite version orders,
one warmup and five measured samples per order, ten pooled samples. Timing was
isolated from builds, tests and profilers.

| Eval case | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| constant | 110.76 | 108.94 | -1.63% |
| property-types-100 | 167.36 | 165.39 | -1.18% |
| property-types-400 | 344.96 | 347.45 | +0.72% |

These small mixed movements do not establish a speedup. Separate heaptrack on
eval/property-400 measured allocations 1,619,611 -> 1,620,060 (+449, +0.03%);
peak heap stayed 29.17 MB. The additional borrowed index has an allocation cost;
replacing a full-table scan does not by itself establish a useful timing gain.

Full workspace passed 364 core, 41 CLI and all remaining tests; release,
diff/source-size checks passed. The new regression checks nested ranges,
overlapping entries, boundary offsets, SourceId separation and empty/missing
ranges. Capture queries now respect source identity, and their search uses one
sorted borrowed index per compilation rather than scanning all evidence for
every closure. Local capture allocation and nested-scope traversal remain.

Evidence: `/tmp/rfc0280-owner-index-{check,test,workspace,release}.log`,
`/tmp/rfc0280-owner-index-{comparison,reverse}.jsonl`,
`/tmp/rfc0280-owner-index-memory-workspace`,
`/tmp/rfc0280-owner-index{,-baseline}-heap.txt`, raw
`/tmp/telora-perf-173/owner-index{,-baseline}.heap.zst`.

The real ontology workload was also rechecked on clean asset revision `1a871a0`.
The original relative `-C ../lab-ws/lab-ontology/ontology` invocation failed before
analysis with "workspace member ontology must be a directory inside the workspace".
Using the same directory's absolute path succeeded without changing any assets:
`-C /home/h00629578/ws/lab-ws/lab-ontology/ontology check @test/query`.
Both versions passed all timed invocations. Opposite orders, one warmup and five
samples per order yielded medians 2.99214 -> 3.01979 seconds (+0.92%). This does
not demonstrate a real-project speedup either. It is an incremental comparison
with `ecb5008`, not the original project baseline. The initial single candidate
run reported 289,608 KiB max RSS; it is not a comparative memory result.
Evidence: `/tmp/rfc0280-owner-index-ontology-comparison.jsonl`,
`/tmp/rfc0280-owner-index-ontology{,-absolute}.{out,log}`.

A separate current-version ontology heaptrack run measured 9,476,222 allocation
calls and 232.88 MB peak heap. This is a new profile, not a before/after memory
comparison. Its allocation stacks still pass through collect_block_annotation_types
and collect_nested_annotation_types into evaluate_tool_expression_with_debug and
infer_tool_expression_evidence. Source inspection confirms block/closure annotations
still execute metadata expressions and decode them into descriptors. Migrating these
remaining annotation consumers to static graph elaboration is the next execution-
boundary investigation; stack presence alone does not quantify its full timing cost.
Evidence: `/tmp/rfc0280-owner-index-ontology-heap.{log,txt}` and
`/tmp/telora-perf-173/owner-index-ontology.heap.zst`.

### Static annotations and partial type solving

Compared the local implementation after `b3ce28b` against its saved release
binary `telora-owner-index`. The candidate is preserved as
`/tmp/telora-perf-173/telora-static-annotations`. Full workspace tests passed
(367 core, 41 CLI including language acceptance, and remaining groups); release,
source-size and diff checks passed. Ontology assets remained clean at `1a871a0`.

Command: `BIN -C /home/h00629578/ws/lab-ws/lab-ontology/ontology check @test/query`.
Both versions succeeded with nine dependencies. Hyperfine used one warmup and
five samples per version, then repeated in reverse binary order. The following
medians pool ten samples per version. No build, tests or profiler ran concurrently
with timing. This is an incremental comparison with `b3ce28b`, not the original
baseline, and check retains its existing value-evaluation behavior.

| Metric | `b3ce28b` | Candidate | Change |
| --- | ---: | ---: | ---: |
| Wall time, median | 2.996063 s | 2.053956 s | -31.44% |
| Wall time, observed range | 2.910164–3.040609 s | 1.981807–2.166379 s | |
| Allocation calls | 9,476,115 | 7,371,280 | -22.21% |
| Peak heap | 232.88 MB | 232.63 MB | essentially unchanged |

Allocation and peak-heap measurements are separate sequential heaptrack runs,
one per version. The lower allocation count does not imply a comparable reduction
in peak live memory. Profiler-inflated runtime and RSS are not timing/memory claims.
No attribution to a single subchange is inferred from the combined candidate.

Raw timing: `/tmp/rfc0280-static-solver-ontology-{forward,reverse}.{json,log}`.
Raw profiles: `/tmp/rfc0280-static-solver-ontology-{baseline,candidate}.heap.zst`;
printed summaries: `/tmp/rfc0280-static-solver-ontology-{baseline,candidate}-heap.txt`.
# Types-only check checkpoint (2026-09-10)

Static tool execution-plan follow-up: owner parameter matching, substitution,
family arities and lowering now produce a VM-free `PreparedToolExpression` before
evaluation. Owner metadata shares the runtime witness graph batch. Open function
shapes read solver slots directly, including proxies; a regression test forbids
descriptor reconstruction for that case. Validation: 377 core regressions, the
new static owner-plan test, and all 42 CLI tests pass. Equivalent ordinary
ontology check, two warmups/five samples per binary: before 2.024 ± 0.011 s,
after 2.025 ± 0.019 s, all exit codes zero. No measurable end-to-end speed change
is established. Baseline `/tmp/telora-tool-plan.xTLPqq/before`; raw data
`/tmp/tool-plan-perf.json` and `/tmp/tool-plan-perf.log`.

Tool evidence graph-only follow-up: runtime type witnesses now contain graph IDs
only; expression records no longer retain compatibility descriptor trees. The
heap batch metadata API consumes `AnalysisTypeId` directly. Core regression (376
tests), the extended generic witness/open-function owner tests, and all 42 CLI
tests pass. Release ontology smoke checks succeeded in both modes: ordinary
internal `check_seconds` 2.033563, pure 0.766981. These are single smoke samples,
not a performance comparison or evidence of additional speedup. Logs:
`/tmp/graph-only-tool-core.log`, `/tmp/graph-only-tool-cli.log`, and
`/tmp/graph-only-tool-release.log`.

Tool inference input separation follow-up: removed runtime-value decoding from
tool-expression inference and extracted a solver without VM/heap/value-binding
parameters. Equivalent-workload release comparison of ordinary ontology
`check @test/query`, two warmups and five measured runs each: before
2.036 ± 0.012 s, after 2.026 ± 0.014 s, all exit codes zero. This does not establish
a clear speed improvement; it removes a runtime dependency needed for moving
tool inference into phase 1. Baseline binary:
`/tmp/telora-pure-tool.Ebzcst/before`; artifacts `/tmp/pure-tool-perf.json` and
`/tmp/pure-tool-perf.log`. Validation: 375 core regression tests, the additional
pure-tool solver test, and all 42 CLI tests (including language acceptance) pass.

Follow-up: shared expression publication now rejects heterogeneous branch results
even when discarded, and semantic interfaces consume the solved TypeGraph without
renumbering. The ordinary path no longer sorts expression records a second time
for its nominal-owner bridge. Core regression: 374 tests passed; the additional
graph-identity preservation test passed separately. Release ontology recheck:
five runs after two warmups per mode, all successful, pure mean 0.7807 ± 0.0112 s,
ordinary mean 2.029 ± 0.023 s. These follow-up measurements show no clear additional
speedup relative to the checkpoint below; this change primarily closes a static
validation gap and establishes graph ownership for downstream consumption.
Artifacts: `/tmp/static-publication-perf.json`, `/tmp/static-publication-perf.log`.

Local `feat/0280-arena-type-consumers` working tree based on `b3ce28b`.
Release build; ontology assets unchanged at `1a871a0`.

Command: `target/release/telora -C /home/h00629578/ws/lab-ws/lab-ontology/ontology check [--types-only] @test/query`.
Hyperfine, two warmups and five measured runs per mode, serial execution with no
concurrent builds/tests. All ten measured commands exited successfully.

| Mode | Mean ± standard deviation | Median | Range |
| --- | --- | --- | --- |
| `check --types-only` | 0.760887 ± 0.014438 s | 0.754703 s | 0.750976–0.786266 s |
| Ordinary `check` | 1.987477 ± 0.004352 s | 1.988734 s | 1.982447–1.993373 s |

These compare different modes of the **same binary**, not a before/after speedup
of equivalent work. Pure checking takes about 38% of ordinary check wall time.
The roughly 1.23 s difference is not an exact phase-2/3 duration: the ordinary
loader still performs its own static preparation and execution, while the pure
loader builds static interfaces and checks reachable builtin source directly.
Their reported dependency counts also differ (16 versus 9) because the static
snapshot includes reachable builtin dependencies.

The pure entry creates no VM/runtime heap and checks value/function bodies,
provider contracts and `@check` callbacks. Regression cases cover non-execution
of division-by-zero module values and failing providers/check callbacks, rejected
type errors, selected imports, newtype constructors and malformed static data.
The ordinary path remains unchanged in execution semantics. Follow-up work must
make it consume the static solution rather than infer again.

Raw artifacts: `/tmp/types-only-ontology-perf.json` and
`/tmp/types-only-ontology-perf.log`. CLI summaries report `catalog_seconds` and
`check_seconds`; these are internal timers, whereas the table reports complete
process wall time.

Compiled tool-plan follow-up: bytecode compilation and external-name resolution
now happen in VM-free preparation; execution consumes the compiled plan and
graph roots. The ordinary metadata scheduler still invokes preparation, so this
does not yet complete the session-wide static/execution boundary. Validation:
380 core tests and all 42 CLI tests (including language acceptance) pass;
release build, source-size check and `git diff --check` pass.

Release ontology comparison, two warmups and five measured runs per command,
without concurrent builds/tests; all commands succeeded and assets remain clean:
ordinary check before 2.024 ± 0.017 s, after 2.022 ± 0.031 s. There is no clear
additional speedup. The same new binary's `check --types-only` takes
0.7654 ± 0.0033 s, about 38% of ordinary check wall time. The 1.26 s difference
is still not an exact measurement of phases 2/3, for the pipeline differences
described above. Baseline: `/tmp/telora-compiled-plan.FcY6FX/before`;
artifacts: `/tmp/compiled-tool-plan-perf.json` and
`/tmp/compiled-tool-plan-perf.log`.

Required tool-evidence follow-up: removed the constructor-presence shortcut
which returned empty evidence for non-function tool expressions. Scalar tool
expressions now always run static inference and publication. Removed the flag
and its descriptor scans; the compiler also consumes the lowered expression
without cloning it again. Validation: 381 core tests, all 42 CLI tests, release
build, source-size check and diff whitespace check pass. A regression checks
scalar success and type errors with and without an expected type, including
`1 / 0` being accepted without evaluation.

Equivalent ordinary ontology checks (two warmups, five runs, no concurrent
builds/tests): before 2.031 ± 0.025 s, after 2.033 ± 0.028 s; all successful.
No measurable speed change. Baseline `/tmp/telora-before-required-tool-evidence`;
artifacts `/tmp/required-tool-evidence-perf.{json,log}`. The ordinary scheduler
still owns inference; removing this shortcut does not complete phase separation.

Property generated-call follow-up: removed the lightweight recorded inference
before generic tool inference, and its supplemental descriptor table. A VM-free
`prepare_property_call` solves against the property result once, restores scoped
static inputs and compiles the plan before the previous runtime value is built.
Provider signature discovery and capability validation remain in the ordinary
scheduler; this does not yet complete session-wide static property planning.
Validation: 382 core tests, all 42 CLI tests (including language acceptance),
release build, source-size and diff checks pass. A new regression verifies
previous-value type injection without runtime resources and scope restoration
after both success and type error.

Equivalent ordinary ontology checks (two warmups and five measured runs each,
no concurrent builds/tests): before 2.035 ± 0.030 s, after 2.004 ± 0.020 s, all
successful. The sample mean is about 1.5% lower, but the difference is small
relative to observed variation; do not treat it as an established speedup.
Baseline `/tmp/telora-before-property-single-solve`; artifacts
`/tmp/property-single-solve-perf.{json,log}`.

Single evidence-source / static check-contract follow-up: removed the remaining
tool-plan descriptor supplementation from lightweight inference. Check dependency
annotations now use static elaboration instead of executing a type expression
and decoding runtime metadata; errors are reported rather than ignored.
Validation: 383 core tests, all 42 CLI tests including language acceptance,
release build, source-size and diff checks pass. New regression covers primitive
and array contracts and rejects a value-level type factory without invoking it.

Equivalent ordinary ontology checks, two warmups and five runs each without
concurrent builds/tests: before 2.036 ± 0.036 s, after 2.035 ± 0.025 s, all
successful. No measurable speedup. Baseline `/tmp/telora-before-static-check-contract`;
artifacts `/tmp/static-check-contract-perf.{json,log}`. Ordinary type declaration
branches still contain execution fallbacks; full static/execution separation
is not established by this follow-up.

Static declaration scheduling follow-up: every type declaration now enters
static scheduling. Removed source-order execution of helper-dependent type
declarations and VM fallbacks for concrete, family and recursive bodies.
Materialization requires a solved graph ID, and recursive descriptor publication
has no evaluator/value input. Removed dead runtime declaration helpers and the
retained reverse dependency table. Invalid Unchecked targets now produce their
precise error during static elaboration; three acceptance fixtures initially
exposed this missing diagnostic after fallback removal and now pass unchanged.

Validation: all 384 core tests and 42 CLI tests (including language acceptance),
release build, source-size and diff checks pass. Zero-fuel cases cover ordinary,
generic and recursive declarations attempting to call a value-level type factory;
these are rejected by the existing static type/value boundary, before body
elaboration. A direct types-only invalid-Unchecked check also reports the precise
static error. Ordinary metadata materialization and tool inference scheduling
remain interleaved; full phase separation is still incomplete.

Equivalent ordinary ontology check comparison, two warmups and five measured
runs each with no concurrent builds/tests: before 1.996 ± 0.019 s, after
2.016 ± 0.023 s, all successful. The roughly 1% higher mean is within the observed
variation; no performance benefit established. Baseline
`/tmp/telora-before-all-static-declarations`; raw data
`/tmp/all-static-declarations-perf.{json,log}`.

Static recursive-family handoff: signature construction, bound validation and
runtime-rebuild classification now precede runtime materialization in a VM-free
helper. The materializer consumes prepared parameters and graph roots, no longer
parses binders twice or returns a TypeScheme. Removed the unused runtime-template
input to static family inventory (empty at every call site). Validation: all
384 core tests and 42 CLI tests including language acceptance, release build,
source-size and diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 2.005 ± 0.012 s, after 1.999 ± 0.018 s,
all successful. No measurable speedup. Baseline
`/tmp/telora-before-static-family-handoff`; raw data
`/tmp/static-family-handoff-perf.{json,log}`. The declaration loop still interleaves
static solving and materialization; full session phase separation remains open.

Nominal graph handoff: concrete declaration identities are now prepared without
a VM, and runtime materialization consumes an immutable graph/root. Nominal
wrapping directly reuses the body ID instead of expanding the body to a descriptor
tree and reimporting it. Regression verifies one added node, unchanged body IDs,
descriptor reimport identity and refinement of a reserved Never-body row.
Validation: all 385 core tests, 42 CLI tests (including language acceptance),
release build, source-size and diff checks pass.

Equivalent ordinary ontology check, two warmups and five runs per command,
without concurrent builds/tests: before 2.021 ± 0.038 s, after 2.025 ± 0.012 s,
all successful. No measurable speedup. Baseline
`/tmp/telora-before-nominal-graph-handoff`; artifacts
`/tmp/nominal-graph-handoff-perf.{json,log}`. This removes one descriptor round trip;
it does not yet defer all materialization until session static solving completes.

Deferred declaration materialization: the declaration loop now emits ordered
graph-root plans for concrete types, families and recursive components. Runtime
placeholders and metadata are created only after declaration solving, definition
contracts, trait-overlap/interpreter checks and unresolved-name validation succeed.
Recursive runtime references/bodies use vectors. The tool inference context is
created once afterward, removing initial environment cloning plus two refreshes
and per-declaration publication. Full function-body inference and tool scheduling
still follow materialization; this is not the final session static boundary.

Validation: all 385 core tests, all 42 CLI tests including language acceptance,
release build, source-size and diff checks pass. Equivalent ordinary ontology
check, two warmups and five measured runs each, without concurrent builds/tests:
before 2.034 ± 0.044 s, after 2.005 ± 0.023 s, all successful. The approximately
1.4% lower sample mean is close to observed variation; no stable speedup established.
Baseline `/tmp/telora-before-deferred-declarations`; artifacts
`/tmp/deferred-declarations-perf.{json,log}`.

Static property-contract collection: ordinary and types-only checking now share
VM-free presence collection from provider signatures and target static types.
Repeated declarations deduplicate by static type identity. Ordinary evidence
linking separately assigns runtime binding names and no longer reads a target
runtime value. Type-ID linking still uses the current evaluator's type store;
full phase separation remains incomplete. Regression covers repeated declarations
without runtime resources. All 386 core tests and 42 CLI tests, release build,
source-size and diff checks pass.

Equivalent ordinary ontology check, two warmups and five runs each with no
concurrent builds/tests: before 1.999 ± 0.020 s, after 2.009 ± 0.013 s, all
successful. No measurable benefit; the difference is close to sample variation.
Baseline `/tmp/telora-before-static-property-contracts`; artifacts
`/tmp/static-property-contracts-perf.{json,log}`.

Native import cleanup: native TypeSchemes are installed with initial descriptor
import; the later binding pass no longer decodes metadata or reconstructs these
schemes. All 386 core tests, 42 CLI tests including language acceptance, release
build, source-size and diff checks pass. No new performance measurement was run
for this cleanup; no speedup is claimed. Logs:
`/tmp/native-type-single-import-{core,cli,release}.log`.

Program solve before tools: split the preliminary binding/environment pass from
tool execution. Ordered tool tasks execute after complete program inference and
expression publication, as do declaration materialization and construction-check
factories. Removed obsolete context synchronization APIs and construction-check
attempts at bindings with no tool work. Validation: 386 existing core tests pass,
plus the new execution-order regression passes separately; all 42 CLI tests,
release build, source-size and diff checks pass. The regression confirms that a
later function type error prevents an earlier failing check factory from running,
and that fixing the type error permits the factory to execute.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 1.994 ± 0.020 s (1.970–2.018), after
1.926 ± 0.010 s (1.916–1.941), all successful. Mean wall time decreases about
3.4%, with non-overlapping sample ranges. Baseline
`/tmp/telora-before-solve-before-tools`; artifacts `/tmp/solve-before-tools-perf.{json,log}`.
This is a per-module improvement: ordinary imported-module execution, independent
tool-expression inference, and final generic normalization still prevent claiming
the session-wide three-phase architecture complete.

Solved tool-input handoff: execution now follows inferred-scheme normalization,
interface publication and publishable-scheme validation. Tool setup consumes
complete normalized binding types and quantified schemes; queued expected types
normalize before materialization. 387 core tests and all 42 CLI tests pass,
along with release build, source-size and diff checks. Independent tool inference
is still present, so this does not finish the phase boundary.

Equivalent ordinary ontology check, two warmups and five measured runs each
without concurrent builds/tests: before 1.928 ± 0.015 s (1.916–1.948), after
1.962 ± 0.018 s (1.943–1.988), all successful. The sample mean is about 1.8%
higher; no performance gain is claimed. This transitional change builds complete
execution inputs while the independent tool inference path still exists; removal
of that repeated solve remains necessary. Baseline
`/tmp/telora-before-solved-tool-inputs`; artifacts `/tmp/solved-tool-inputs-perf.{json,log}`.

Main-solver evidence reuse: normal top-level tool tasks now select expression,
constructor, call, trait/interpolation and lexical evidence from the completed
program solver and compile plans before materialization. Their execution no
longer reinfers the expression; missing evidence fails without a reinference
fallback. The old typed-tool evaluation wrapper is removed. All 387 core tests
and 42 CLI tests including language acceptance, release build, source-size and
diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 1.965 ± 0.020 s, after 1.967 ± 0.037 s,
all successful. No measurable improvement. Baseline
`/tmp/telora-before-reuse-program-evidence`; artifacts
`/tmp/reuse-program-evidence-perf.{json,log}`. Construction-check dependency
retries and generated property/check expressions still have independent inference;
plans also still publish their own type graphs. These remain required follow-ups.

Reusable construction dependency plans: retries now borrow the same precompiled
top-level plans as ordinary tool tasks, without annotation elaboration, independent
inference or recompilation. Missing external bindings are checked before runtime
type materialization. A regression executes the same plan with different bindings
and no inference context. All 387 core tests, 42 CLI tests, release build,
source-size and diff checks pass.

Two warmups and five measured runs per command, with no concurrent builds/tests:
ordinary ontology check before 1.986 ± 0.008 s, after 1.974 ± 0.025 s. The small
difference does not demonstrate a performance gain. The same release binary's
`check --types-only @test/query` takes 0.767 ± 0.009 s, about 39% of ordinary
check wall time. These are separate end-to-end paths, not an exact decomposition
of ordinary check into static/tool/runtime phases; ordinary check does not yet
consume the pure static entry's complete artifact. Generated check/property
expressions and per-plan type graphs remain follow-ups. Baseline
`/tmp/telora-before-reusable-tool-plans`; artifacts
`/tmp/reusable-tool-plans-perf.{json,log}`.

Definition-local tool results: each top-level tool task now retains its successful
value. Construction dependency scheduling and the normal task loop consume the
same completed slot instead of executing the definition twice. Failed attempts
leave the slot empty; the runtime name map is not used as completion identity.
The new regression verifies missing-input retry followed by successful result
reuse with no inputs and zero fuel. All 388 core tests, 42 CLI tests including
language acceptance, release build, source-size and diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 1.960 ± 0.031 s (1.929–2.000), after
1.973 ± 0.013 s (1.958–1.992), all successful. No measurable speedup; mean wall
time is about 0.7% higher and sample ranges overlap. Baseline
`/tmp/telora-before-tool-definition-once`; artifacts
`/tmp/tool-definition-once-perf.{json,log}`. Generated construction-check and
property plans still require migration out of execution-time inference.

Static construction-check plans: `prepare_construction_checks` validates nominal
targets and check contracts and compiles every check expression once, before
declaration materialization/tool execution. This function accepts no evaluator,
VM or heap. The execution/retry path consumes prepared plans and parameter IDs;
it no longer solves expressions or defers type errors to an execution retry.
The regression validates both valid/invalid contracts before creating a VM and
then registers a valid plan with no evaluator inference context. All 389 core
tests, 42 CLI tests including language acceptance, release build, source-size
and diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 1.985 ± 0.037 s (1.964–2.052), after
1.976 ± 0.030 s (1.947–2.023), all successful. No measurable speedup. Baseline
`/tmp/telora-before-static-check-plans`; artifacts
`/tmp/static-check-plans-perf.{json,log}`. Check preparation still performs one
independent static expression solve rather than consuming main-solver evidence;
property preparation still runs at execution time. The complete session-wide
static artifact and removal of evaluator inference context remain outstanding.

Static property plans and evaluator separation: capability expressions and
generated provider calls now prepare before declaration materialization and tool
execution. Provider plans retain their nominal result descriptor, avoiding
repeated contract elaboration in reduce. Runtime capability evaluation/validation
and chained property values retain their existing behavior. Deleted
`ToolEvaluator.inference_context` and the evaluator-based inference wrapper; the
temporary static tool context is dropped before execution. A new test prepares
a complete property module with no capability/provider values or VM. All 390
core tests, 42 CLI tests including language acceptance, release build, source-size
and diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests: before 1.967 ± 0.013 s (1.947–1.983), after
1.948 ± 0.025 s (1.917–1.982), all successful. Mean wall time is about 1% lower,
but overlapping ranges do not establish a clear speedup. Baseline
`/tmp/telora-before-static-property-plans`; artifacts
`/tmp/static-property-plans-perf.{json,log}`. Generated expressions still have
independent static solves, plans retain separate type graphs, property execution
sites link plans by source location, and ordinary module loading still needs the
complete session-wide static artifact. This is a per-module execution boundary,
not completion of session-wide three-phase separation.

Tool graph retention: after compilation, runtime witness roots are filtered by
the compiler's actual external links. Plans with no remaining runtime type roots
release their static expression graph; plans with runtime roots preserve existing
IDs without descriptor rebuilding. A regression injects unused named-type
evidence and verifies it is discarded, the graph is empty and execution still
returns 42. All 391 core tests, 42 CLI tests, release build, source-size and diff
checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs each,
without concurrent builds/tests/profilers: before 1.973 ± 0.008 s, after
1.962 ± 0.015 s, overlapping ranges and no demonstrated speedup. Separate
heaptrack runs report 7,339,445 → 7,339,519 allocation calls and 232.62 →
232.63 MB peak heap consumption: no observable allocation or peak-memory benefit
on this workload. Profiler runtime and RSS are not performance claims. Baseline
`/tmp/telora-before-tool-graph-retention`; timing artifacts
`/tmp/tool-graph-retention-perf.{json,log}`; heap artifacts
`/tmp/tool-graph-retention-{before,after}.zst` and corresponding `-summary.log`.
This reduces unnecessary graph lifetime but does not eliminate the earlier graph
construction. Shared graph publication and session-wide consumption remain the
larger unresolved work.

Shared module tool arena and materialized slots: top-level plans, generated
construction-check plans and property plans publish directly into one TypeGraph.
Evidence and compiled plans no longer own separate graphs. The completed arena
moves into the evaluator once, without graph merging or ID remapping. Metadata
materialization uses a persistent graph-ID-indexed table within the same immutable
graph/work-heap lifetime; repeated roots return the same values. Failure clears
the table to prevent provisional recursive owners escaping as completed results.
Tests cover node/ID reuse across plans, materialized value identity, and repeated
failure followed by valid materialization. All 392 core tests, 42 CLI tests,
release build, source-size and diff checks pass; the strengthened identity test
also passes separately.

The initial graph-only checkpoint measured 1.966 ± 0.017 → 2.021 ± 0.061 s;
inspection identified per-call graph-sized conversion tables and motivated the
persistent materialization slots. Final equivalent ordinary ontology check,
two warmups and five measured runs each, without concurrent builds/tests/profilers:
before the combined change 1.960 ± 0.019 s (1.934–1.985), after 1.967 ± 0.041 s
(1.931–2.018). No demonstrated speedup. Separate heaptrack runs report
7,339,494 → 7,339,549 allocation calls and 232.62 → 232.62 MB peak heap:
no observable allocation or peak-memory benefit on ontology. Profiler runtime/RSS
are not performance claims.

Baseline `/tmp/telora-before-shared-tool-arena`; final timing artifacts
`/tmp/shared-tool-materialization-perf.{json,log}`; heap baseline
`/tmp/shared-tool-arena-before.zst`, final heap
`/tmp/shared-tool-materialization-after.zst`, corresponding `-summary.log` files.
Intermediate graph-only binary `/tmp/telora-shared-tool-arena-only` and
`/tmp/shared-tool-arena-*` artifacts retain that checkpoint. The main module
analysis graph and session-wide artifact are still separate from this tool arena;
independent generated-expression solving and main-evidence publication remain.
The final allocation report (`/tmp/shared-tool-materialization-allocators.log`)
still shows `TypeDescriptor::clone`/BTreeMap cloning beneath
`infer_expr_recorded`/`infer_expr_with` in the ordinary analyzer. String cloning
accounts for 3,621,187 allocation calls across all its callers; this is not a
claim that the entire count belongs to that one inference path. The remaining
lightweight expression-descriptor pass is a concrete next investigation target.

Remove provisional expression recording: the preliminary binding pass now asks
only for root types and does not clone every subexpression descriptor into a
parallel map. Provisional annotation/result traversals and property-contract
throwaway recording maps are removed. Downstream expression types come from
main-solver graph publication, with nominal bridges built from solved records.
The initial validation exposed import path literals missing from that publication;
the main solver now explicitly records their String type. The semantic coverage
test remains strict and no provisional-record fallback is retained.

All 392 core tests, 42 CLI tests including language acceptance, release build,
source-size and diff checks pass. Equivalent ordinary ontology check, two warmups
and five measured runs each, without concurrent builds/tests/profilers:

| Metric | Before this step | After | Change |
| --- | ---: | ---: | ---: |
| Mean wall time | 1.985 ± 0.033 s | 1.258 ± 0.016 s | -36.6% |
| Sample range | 1.953–2.035 s | 1.236–1.278 s | non-overlapping |
| Allocation calls | 7,339,458 | 5,374,078 | -26.8% |
| Peak heap | 232.62 MB | 118.09 MB | -49.2% |

Heaptrack ran separately from timing. Profiler runtime and RSS are not used for
these claims. This is an incremental comparison with the immediately preceding
shared-tool-arena/materialization version, not the original RFC baseline.
Baseline `/tmp/telora-before-projection-record-removal`; timing artifacts
`/tmp/projection-record-removal-perf.{json,log}`; heap artifacts
`/tmp/projection-record-removal-{before,after}.zst` and corresponding
`-summary.log` files. Preliminary root-type projection and descriptor-based
contract/environment consumers still remain; complete session graph unification
is not finished by this change.

Skip discarded contracted-body projections: the preliminary pass no longer
projects `impl` bodies or `def` bodies whose type is already supplied by an
established contract. Those branches previously computed a projection eagerly
and discarded it. The complete program solver still checks the bodies; only
definitions without contracts retain provisional root projection.

All 392 core tests (including type-error-before-tool-execution coverage), 42 CLI
tests including language acceptance, release build, source-size and diff checks
pass. Equivalent ordinary ontology check, two warmups and five measured runs per
version without concurrent builds/tests/profilers:

| Metric | Before this step | After | Change |
| --- | ---: | ---: | ---: |
| Mean wall time | 1.278 ± 0.011 s | 1.184 ± 0.026 s | -7.4% |
| Sample range | 1.263–1.292 s | 1.160–1.228 s | non-overlapping |
| Allocation calls | 5,374,119 | 4,347,785 | -19.1% |
| Peak heap | 118.09 MB | 118.09 MB | unchanged |

Heaptrack ran separately; profiler runtime/RSS are not performance claims. This
comparison is with the immediately preceding provisional-record-removal version,
not the original RFC baseline. Baseline `/tmp/telora-before-contract-projection`;
artifacts `/tmp/contract-projection-perf.{json,log}` and
`/tmp/contract-projection-{before,after}.zst` with corresponding `-summary.log`
files. Uncontracted projection, descriptor-based environments and session graph
unification remain outstanding.

Root-only projection: removed the obsolete expression-recording callback and
discarded traversal of operands that cannot influence the provisional root type.
Known function results no longer project arguments; comparison/string results,
return/fail, index results and other forms avoid discarded work. Full checking
remains in the program solver. New regressions ensure projection does not read
irrelevant operand types and that complete solving still rejects invalid call
arguments. All 394 core tests and 42 CLI tests pass, along with release build,
source-size and diff checks. An overlapping acceptance run collided in the shared
generated-output directory; the final CLI suite was rerun serially in isolation
and passed.

Equivalent ordinary ontology check, two warmups and five measured runs per
version, with no concurrent builds/tests/profilers: before 1.166 ± 0.003 s
(1.161–1.170), after 1.154 ± 0.018 s (1.136–1.182). No clear timing gain because
sample ranges overlap. Separate heaptrack runs show 4,347,819 → 4,117,896
allocation calls (-5.3%) and 118.09 → 118.08 MB peak heap (essentially unchanged).
Profiler runtime/RSS are not performance claims. Baseline
`/tmp/telora-before-root-only-projection`; artifacts
`/tmp/root-only-projection-perf.{json,log}` and
`/tmp/root-only-projection-{before,after}.zst` with corresponding `-summary.log`
files. This is incremental to contracted-body projection removal. The remaining
root projection is still transitional and must eventually be replaced by main
solver evidence.

Open closure projection: once any parameter lacks a provisional type, closure
projection now returns unknown immediately instead of traversing the entire body
before inevitably discarding its result. The main solver still infers the full
closure. The projection regression verifies that this path never reads an
irrelevant body type. All 394 core tests, 42 serial CLI tests including language
acceptance, release build, source-size and diff checks pass.

Equivalent ordinary ontology check, two warmups and five measured runs per
version, without concurrent builds/tests/profilers: before 1.125 ± 0.009 s,
after 1.128 ± 0.004 s; no observable timing gain. Separate heaptrack runs show
4,117,920 → 4,115,345 allocation calls (2,575 fewer, about 0.06%) and
118.09 → 118.09 MB peak heap. This is not a material memory improvement on
ontology. Profiler runtime/RSS are not performance claims. Baseline
`/tmp/telora-before-open-closure-projection`; artifacts
`/tmp/open-closure-projection-perf.{json,log}` and
`/tmp/open-closure-projection-{before,after}.zst` with corresponding `-summary.log`
files. This result reinforces the need to focus next on larger solver/environment
construction paths rather than expecting further large gains from local
projection traversal cleanup.

Main-solver-only definitions (withdrawn after the provider-contract regression
described below): removed root projection for every `def`, including
definitions without annotations. Established contracts remain static inputs;
uncontracted definitions now rely directly on the main solver's dependency-ordered
slots. Preliminary `let` diagnostics and property-contract projection remain.
All 394 core tests, 42 CLI tests including language acceptance, release build,
source-size and diff checks pass; ontology check succeeds.

Timing used two warmups and five runs per binary in each of forward/reverse
order, ten samples per binary in total. The reverse run followed an outlier
warning on the first baseline sample set. No builds/tests/profilers overlapped
timing. Equivalent ordinary ontology check, pooled samples:

| Metric | Before this step | After | Change |
| --- | ---: | ---: | ---: |
| Median wall time | 1.14136 s | 1.10022 s | -3.6% |
| Sample range | 1.12139–1.20624 s | 1.08569–1.11816 s | non-overlapping |
| Allocation calls | 4,115,267 | 3,701,935 | -10.0% |
| Peak heap | 118.09 MB | 117.70 MB | -0.3% |

Heaptrack ran separately; profiler runtime/RSS are not performance claims.
Baseline `/tmp/telora-before-main-definition-solve`; timing artifacts
`/tmp/main-definition-solve-perf{,-reverse}.{json,log}`; heap artifacts
`/tmp/main-definition-solve-{before,after}.zst` and corresponding `-summary.log`
files. This is incremental to open-closure projection short-circuiting, not the
original RFC baseline. Session graph unification and the remaining descriptor
environment/contract consumers are still outstanding.

Correction: the main-solver-only definitions experiment above broke valid
property provider aliases and typed factories returning providers. Property
contract discovery still precedes the main solver, so unannotated definitions
must retain root projection until discovery consumes solved graph evidence.
The removal was reverted; the claimed 3.6% timing and 10.0% allocation gains
are withdrawn. Earlier safe projection optimizations remain in place.
A new CLI regression covers both direct provider aliases and factory results.
All 394 core tests and 43 serial CLI tests, including language acceptance, pass;
release build, source-size and diff checks pass.

Repaired release versus the last valid pre-experiment binary, with two warmups
and five runs and no concurrent builds/tests/profilers: 1.132 ± 0.007 s before,
1.127 ± 0.015 s repaired. This is no measurable timing improvement. No fresh
memory claim is made. Artifacts: `/tmp/provider-contract-regression-perf.json`
and `/tmp/provider-contract-regression-perf.log`; baseline binary
`/tmp/telora-before-main-definition-solve`.

Current `check --types-only` comparison after the correction: two warmups and
five measured runs per mode, no overlapping builds/tests/profilers. Pure static
check takes 0.7729 ± 0.0094 s; ordinary check takes 1.126 ± 0.016 s on the
ontology `@test/query` workload. Artifact: `/tmp/types-only-current-perf.json`.
These are separate command paths, not exact phase accounting: the difference
must not be presented as isolated metadata/runtime execution time. Both JSON
summaries expose `catalog_seconds`, `check_seconds`, and `types_only`.

Shared tool-binding publication: all main-solver tool bindings now reuse one
slot-to-graph publication table against their shared module tool arena. No table
is allocated when there are no tool bindings. The table is dropped before the
independent generated-expression solvers run. This removes a per-binding
full-solver table allocation and repeated publication of shared slots; it does
not unify the main and tool graphs or remove per-binding evidence-map scans.

Validation: 394 core tests and 43 serial CLI tests including language acceptance
passed. After the lazy-allocation refinement, all five publication-specific tests
passed; release build, source-size and diff checks passed. Equivalent ordinary
ontology check, two warmups and five runs per binary with no overlapping builds,
tests or profilers: before 1.141 ± 0.023 s, after 1.159 ± 0.036 s. The overlapping
samples do not establish a timing improvement. Separate heaptrack runs measured
4,115,381 → 4,115,289 allocation calls (92 fewer) and 118.09 → 118.08 MB peak
heap: no material memory improvement on this workload. Profiler runtime/RSS are
not performance claims. Baseline `/tmp/telora-before-shared-tool-publication`;
artifacts `/tmp/shared-tool-publication-perf.{json,log}` and
`/tmp/shared-tool-publication-{before,after}.zst` with corresponding summary logs.

Deferred trait evidence: explicit member calls now infer from the trait's
declared signature and select implementations at the final constraint gate.
Interpolation likewise records a lexical obligation and resolves Display evidence
at finalization. This removes eager property-dependent candidate selection from
expression inference; local provider contracts still need to migrate before the
gate. There is no second expression inference pass or execution fallback.
A regression infers `fn(x) { Combine.combine(x, 1) }` as `Fn(Int) -> Int` from
the second argument, verifies execution returns 42, and rejects a missing impl.
All 395 core tests and 43 serial CLI tests, including language acceptance, pass.
Release build, source-size and diff checks pass; ontology types-only check succeeds.

Equivalent ordinary ontology check, two warmups and five runs per binary after
all tests/builds completed: before 1.138 ± 0.010 s, after 1.134 ± 0.012 s; no
observable speedup. Separate heaptrack runs measured 4,115,318 → 4,115,306
allocations and 118.09 → 118.08 MB peak heap, also no material improvement.
Profiler runtime/RSS are not performance claims. Baseline binary:
`/tmp/telora-before-deferred-trait-evidence`; artifacts:
`/tmp/deferred-trait-perf.{json,log}`, `/tmp/deferred-trait-{before,after}.zst`
and corresponding summary logs. This is a prerequisite for moving provider
contract discovery onto solved evidence, not a claimed performance milestone.

Main-solver property contracts: ordinary and types-only checking now infer provider
expressions against the completed program binding slots, validate their nominal
return contracts, and install local property-presence evidence before finalizing
trait/property obligations. Evidence roots use module ID plus contract ordinal,
without VM/heap access. Execution fills those exact roots and validates the
materialized property set against the static contracts. Unannotated `def` root
projection is removed; `let` projection remains. This supersedes the previously
withdrawn removal by migrating its property-contract dependency first.

The CLI regression now covers direct aliases, typed factories, inferred factory
return types, direct property constraints and property-dependent trait calls in
both check modes. All 395 core tests and the other 42 CLI tests passed. Language
acceptance initially found one changed diagnostic: invalid rename_all configuration
now reports `cannot unify String with RenameCase` at the argument from the main
solver. Its expected text was updated, and the complete language acceptance test
then passed. Release build, source-size and diff checks passed; ordinary and
types-only ontology checks succeeded.

Equivalent ordinary ontology check, two warmups and five runs per version in
each of forward/reverse order (ten samples per binary), without concurrent
builds/tests/profilers. Forward means: 1.149 ± 0.008 → 1.153 ± 0.057 s; reversed:
1.139 ± 0.020 → 1.114 ± 0.037 s. Pooled mean 1.14379 → 1.13331 s, median
1.14141 → 1.10732 s. The changed binary has a wide 1.08483–1.21234 s range, so
these samples do not establish a stable timing improvement.

Separate heaptrack runs: 4,115,324 → 3,701,963 allocation calls (413,361 fewer,
10.04% reduction); peak heap 118.09 → 117.69 MB (about 0.34% lower). Allocation
traffic falls materially; peak memory does not. Profiler runtime/RSS are not
performance claims. Baseline `/tmp/telora-before-solved-property-contracts`;
artifacts `/tmp/solved-property-perf{,-reverse}.{json,log}` and
`/tmp/solved-property-{before,after}.zst` with corresponding summary logs.

Property contract consumers: main inference now records contracts for type, field
and variant decorators. Tool-plan preparation and the remaining types-only
decorator validation require these records, with no reprojection or fallback.
The old decorator projection helper is deleted. Member provider expressions are
checked in main inference before evidence finalization; they do not establish
type-level property-presence facts. Synthetic chained calls still have their own
static inference and remain a migration target.

All 395 core tests and 44 serial CLI tests, including language acceptance, pass.
A new field-provider regression succeeds in types-only mode and deliberately
fails inside the provider during ordinary check, proving that static contract
checking does not execute it. The plan test rejects a missing contract even when
its environment could supply the provider type. Release build, source-size and
diff checks pass; both ontology check modes succeed.

Equivalent ordinary ontology check, two warmups and five runs per binary without
overlapping builds/tests/profilers: 1.090 ± 0.009 → 1.097 ± 0.018 s. No observable
speedup. Separate heaptrack: 3,701,921 → 3,701,902 allocation calls (19 fewer),
117.70 → 117.69 MB peak heap; no material memory improvement. Profiler runtime
and RSS are not performance claims. Baseline:
`/tmp/telora-before-property-contract-consumers`; artifacts:
`/tmp/property-contract-consumers-perf.{json,log}` and
`/tmp/property-contract-consumers-{before,after}.zst` with corresponding summaries.

Unified module type arena: the main module graph now moves into tool preparation,
then the evaluator, then back into Analysis. Tool plans and main expressions use
the same graph IDs without graph copying, merging, remapping or Arc. Their
publication table is also shared across the handoff instead of rebuilding a
second table and revisiting the main solver's slots. The graph is frozen during
execution. This unifies one module's arenas, not the session's module graphs;
generated expressions still have independent static solvers.

All 395 core and 44 serial CLI tests including language acceptance pass, along
with release build, source-size and diff checks. The expanded graph handoff test
checks that preparation moves the original node storage, uses a preexisting
module root ID in multiple plans, reuses its materialized value, and returns the
graph with both that root and an unrelated original root intact. Both ontology
check modes succeed.

Equivalent ordinary ontology check, two warmups and five measured runs per
binary without concurrent builds/tests/profilers: 1.080 ± 0.008 → 1.088 ± 0.009 s,
no observable speedup. Separate heaptrack runs measured 3,701,912 → 3,701,547
allocation calls (365 fewer, about 0.01%) and unchanged 117.69 MB peak heap.
No material memory improvement. Profiler runtime/RSS are not performance claims.
Baseline `/tmp/telora-before-unified-module-type-graph`; artifacts
`/tmp/unified-module-type-graph-perf.{json,log}` and
`/tmp/unified-module-type-graph-{before,after}.zst` with corresponding summaries.

### Types-only CLI verification (2026-09-10)

Current working-tree release, ontology `@test/query`, two warmups and five
measured runs per mode, with all builds and tests finished before measurement:

| Mode | Wall time (mean ± standard deviation) |
| --- | --- |
| `check --types-only` | 848.4 ± 20.4 ms |
| `check` | 1165 ± 15 ms |

These compare two modes of the same binary, not optimization checkpoints.
Their difference is not precise tool/runtime phase time: the static and full
workspace paths have not yet been unified. JSON summaries report
`catalog_seconds` separately from `check_seconds`; the latter measures the
selected check path. Both modes succeed. Artifacts:
`/tmp/types-only-comparison.{json,log}`, `/tmp/types-only-query.jsonl`, and
`/tmp/full-check-query.jsonl`.

Validation: 397 core tests and 44 CLI tests (including language acceptance),
release build, source-size and diff checks pass. Annotation ingress fixes retain
nominal body lookup through imported slots, avoid treating concrete Dyn targets
as symbolic slot handles, and allow structural coalescing across nominal
recursion boundaries. The recursive checked-tree acceptance case passes all
five checks. This verification does not establish completion of the broader
session-wide solver migration.

### Recursive inference slot publication (2026-09-10)

Publication validation now traverses slot edges once and propagates actual
failure bits through a flat reverse-edge table to a fixed point. Revisiting a
recursive edge no longer invents an unresolved type. Publication reserves graph
IDs for recursive references, including structural ancestors of nominal nodes,
then fills them without descriptor expansion. Regression coverage checks both
root orders, complete recursive edges, absence of pending published nodes and
descriptor views, and propagation of an unknown through a cycle into cached
validation results.

398 core tests and 44 CLI tests including language acceptance pass; release,
source-size and diff checks pass. Ordinary ontology check, two warmups and five
measured runs with no concurrent builds/tests: 1.181 ± 0.007 s before and
1.183 ± 0.034 s after. No observable speedup or slowdown. This is a correctness
prerequisite for consuming cyclic arena IDs, not a measured performance gain.
Baseline `/tmp/telora-before-cyclic-publication`; measurements
`/tmp/cyclic-publication-perf.{json,log}`. Remaining descriptor bridges and the
session-wide phase boundary remain open.

### Annotation ingress cost audit and traversal cleanup (2026-09-10)

Re-measured the last pre-annotation-ingress release against the cyclic-publication
checkpoint, using ordinary ontology `@test/query`, two warmups and five measured
runs per binary with no concurrent builds/tests/profilers:
1.134 ± 0.041 → 1.181 ± 0.007 s. This is not a speedup; the direction suggests
regression, although the baseline is noisy. Separate heaptrack recordings show
3,701,571 → 4,755,362 allocation calls (+28.5%), while peak heap drops from
117.69 to 89.96 MB (-23.6%). Do not describe this migration as an unconditional
performance improvement.

Filtering allocation backtraces containing `normalize` and summing all reported
allocator groups gives 1,054,233 → 2,454,141 inclusive allocation calls. These are
allocation counts, not CPU percentages, and overlap other inclusive scopes.
The increase of about 1.4 million explains why removing graph-to-descriptor
annotation ingress did not improve total allocation counts. The remaining
descriptor consumers repeatedly normalize the newly shared recursive slots.
The next priority is moving those consumers to slot/graph queries, not merely
optimizing import traversal or reinstating eager descriptor trees.

The importer now visits children without allocating a Vec per node, reuses its
validation stack and argument buffer, returns immediately for already imported
roots, and compresses alias edges after connecting the complete chain. A
16,384-alias regression verifies direct proxies to the final root without
descriptor views. 399 core and 44 CLI tests, including language acceptance,
release build, source-size and diff checks pass; both ontology check modes pass.

This cleanup alone changes ordinary check from 1.185 ± 0.010 to
1.177 ± 0.014 s: no observable speedup. Allocation calls fall only from
4,755,362 to 4,754,179 (1,183 fewer), with unchanged 89.96 MB peak heap.
Artifacts: `/tmp/annotation-ingress-perf.{json,log}`,
`/tmp/annotation-ingress-{baseline,current}.zst`,
`/tmp/annotation-ingress-allocations.log`, `/tmp/import-traversal-perf.{json,log}`,
and `/tmp/import-traversal-after.zst`. Binaries:
`/tmp/telora-before-graph-annotation-inputs`, `/tmp/telora-before-import-traversal`.

### Collection consumers read slot shapes (2026-09-10)

Array, tuple, Dict and record literal expectations and spread consumers now read
outer constructors and retain child inference slots instead of normalizing
complete nested descriptor trees. Pending alternatives still perform their
semantic join; retaining this distinction preserves the generic heterogeneous
array diagnostic (`diag-generic-union-context`). A 16,384-layer regression checks
that an outer record query retains the same child slot across solving and
creates no descriptor views. 400 core tests, 44 CLI tests including language
acceptance, release, source-size and diff checks pass. Both ontology modes pass.

Ordinary ontology check, two warmups and five measured runs per binary:
before 1.165 ± 0.009 s, after 1.189 ± 0.020 s. Reversing order gives after
1.183 ± 0.015 s, before 1.204 ± 0.019 s. The direction reverses with order;
there is no stable speedup. Allocations decrease from 4,754,179 to 4,732,679
(21,500 fewer, 0.45%); peak heap changes from 89.96 to 89.92 MB.

A streaming aggregation of complete allocation stacks (no large intermediate
stack file) attributes each allocation containing `normalize` to the caller of
its outermost matching frame. Total 2,409,406 calls, with top callers:

| Caller | Allocation calls |
| --- | ---: |
| `GenericInference::expose_named` | 1,292,803 |
| `GenericInference::infer_inner` | 363,337 |
| `GenericInference::infer_block` | 284,572 |
| `GenericInference::contextualize_authored_literal` | 212,264 |

These are allocation counts, not CPU percentages; inlining limits source-line
attribution. The next larger target is `expose_named`, which currently fully
normalizes before following names and completing nominal bodies. Its consumers
must distinguish shape/identity queries from requests for final descriptors.

Baseline `/tmp/telora-before-collection-shape`; artifacts
`/tmp/collection-shape-perf{,-reverse}.{json,log}`,
`/tmp/collection-shape-after.zst`, `/tmp/collection-shape-after-summary.log`,
`/tmp/collection-shape-normalize-callers.log` and the aggregation script
`/tmp/telora-normalize-callers.awk`.

### Field projection consumes inference slots (2026-09-10)

`project_field` no longer calls `expose_named` and normalizes the complete
receiver. It follows names and nominal body edges, selects record fields by
their sorted row names, and returns the original field slot (or Dict element
slot). Unknown receivers retain one shared field obligation. Recursive stubs
complete their identity through the existing body table without expanding the
body. Pending alternatives still perform their semantic join, and Unchecked
still derives its body from its solved argument. These exceptional semantics
are not treated as ordinary structural edges.

The regression verifies that projected fields retain their slot after solving,
recursive siblings return the original nominal slot, missing fields diagnose,
stubs resolve through the body table, and these ordinary graph queries create
no descriptor views. 401 core and 44 CLI tests including language acceptance
pass; release, source-size and diff checks pass. Both ontology modes pass.

Ordinary ontology `@test/query`, two warmups and five measured runs, without
concurrent builds/tests/profilers: 1.169 ± 0.007 → 1.098 ± 0.009 s (6.1% less
time). Reversed order: after 1.096 ± 0.009, before 1.162 ± 0.014 s (5.7% less).
The improvement persists in both orders. Separate heaptrack recordings show
4,732,679 → 4,190,435 allocations (542,244 fewer, 11.5%). Peak heap is
89.92 → 90.03 MB, no meaningful peak-memory improvement. Comparisons are with
the immediately preceding collection-shape checkpoint, not the original RFC
baseline; allocation count remains above the pre-annotation-ingress checkpoint.

Baseline `/tmp/telora-before-field-slot`; artifacts
`/tmp/field-slot-perf{,-reverse}.{json,log}`, `/tmp/field-slot-after.zst`,
`/tmp/field-slot-after-summary.log`, `/tmp/field-slot-types-only.jsonl`.
Other `expose_named` consumers and the session-wide phase boundary remain open.

### Shallow nominal context and arena row reuse (2026-09-10)

Expression expectation classification and struct-update field discovery now
use `declared_context`. Non-nominal rows are rejected by constructor without
expanding children; nominal contexts normalize identity arguments while retaining
the shared shallow body. Ordinary expression returns skip the normalization
that only literal/atom construction needs. Unchecked and pending-alternative
semantics remain explicit. The deep regression checks that solving an identity
argument changes the returned identity without replacing the shared body or
materializing its 16,384-layer child.

The initial shallow-context change increased allocation calls from 4,190,435 to
4,419,791; its first timing comparison was 1.097 ± 0.014 → 1.114 ± 0.007 s,
and reversed order was after 1.115 ± 0.022, before 1.105 ± 0.020 s. This
intermediate version is not a performance improvement. The allocation diff
included 107,354 additional allocations in `initialize_known`.

Two related reconstruction/initialization costs were then removed. Re-importing
a body view already owned by the solver reuses its immutable type row rather
than lowering that row again. Small argument lists register dependencies by
direct slice inspection; large lists retain sorted deduplication. A conflict
propagation queue is allocated at initialization only when a child is already
conflicted. The row-reuse regression verifies both unchanged row count and live
conflict propagation through the shared children.

Final ordinary ontology comparison against the preceding field-slot checkpoint:
1.119 ± 0.020 → 1.110 ± 0.012 s (two warmups, five measured runs, no concurrent
builds/tests/profilers): no observable speedup. Allocation calls are
4,190,435 → 4,189,613 (822 fewer, effectively unchanged); peak heap is
90.03 → 88.88 MB (1.15 MB lower). These final figures supersede the intermediate
shallow-context result for the delivered working tree. No claim that all
descriptor-consumer costs or the session-wide migration have been eliminated.

403 core tests, 44 CLI tests including language acceptance, release build,
source-size and diff checks pass. Both ontology check modes pass. Baseline:
`/tmp/telora-before-declared-context`. Final artifacts:
`/tmp/declared-context-complete-perf.{json,log}`,
`/tmp/declared-context-complete.zst`, `/tmp/declared-context-complete-summary.log`,
and `/tmp/declared-context-complete-types-only.jsonl`. Intermediate investigation:
`/tmp/declared-context-after.zst`, `/tmp/declared-context-allocation-diff.log`,
`/tmp/declared-context-perf{,-reverse}.{json,log}`.

### Nominal constructor and argument queries use slots (2026-09-10)

Nominal presence/constructor comparisons now follow slot and name edges without
creating descriptor views or complete identity objects. Same-constructor
unification and assignment obtain the argument slots directly instead of
assembling two `DeclaredTypeId` values. The full identity adapter also reads
ordinary nominal rows without materializing their bodies. Arguments remain
inference references, not prematurely claimed final type IDs; Unchecked retains
its explicit argument-derived identity semantics.

The regression verifies constructor lookup, identity argument slots, and mixed
slot/descriptor argument matching without any body views. 403 core tests pass,
along with the updated targeted regression, 44 CLI tests including language
acceptance, release build, source-size and diff checks. Both ontology modes pass.

Ordinary ontology `@test/query`, two warmups and five measured runs per binary,
without concurrent builds/tests/profilers: 1.090 ± 0.009 → 1.091 ± 0.008 s,
no observable timing change. Allocation calls change from 4,189,613 to 4,189,078
(535 fewer); peak heap remains 88.88 MB. This removes more descriptor-facing
queries but does not provide a meaningful measured performance improvement.
Baseline `/tmp/telora-before-nominal-head`; artifacts
`/tmp/nominal-head-perf.{json,log}`, `/tmp/nominal-head-after.zst`,
`/tmp/nominal-head-after-summary.log`, `/tmp/nominal-head-types-only.jsonl`.
Full identity publication and remaining expression/pattern descriptor consumers
are still separate work, as is the session-wide phase boundary.

### Block boundaries retain inference slots (2026-09-10)

The updated streaming stack audit of the nominal-query checkpoint attributes
1,999,207 allocations to stacks containing `normalize`. Largest outer callers
are `infer_inner` (1,080,992), `infer_block` (286,290),
`contextualize_authored_literal` (207,819), and `expose_named` (203,345).
These are allocation counts, not CPU percentages. The hotspot has shifted away
from name exposure toward expression consumers.

Non-generalized local definitions and block results now retain their original
inference slots instead of normalizing a descriptor tree and importing it again
at the enclosing expression. Both local and program-level monomorphic-binding
checks traverse slots to locate remaining unknowns owned by that scope.
Diagnostics still format normalized types, and pending alternatives retain
their semantic join. Nominal definitions keep their separate declaration check
boundary, matching the previous predicate semantics.

403 core tests and 44 CLI tests including language acceptance pass. Expanded
query tests compare the scope predicate against normalization before and after
solving and at different slot thresholds; deep shared graph checks create no
descriptor views. Release, source-size and diff checks pass; both ontology check
modes pass.

Ordinary ontology `@test/query`, two warmups and five measured runs per binary
without concurrent builds/tests/profilers: 1.095 ± 0.021 → 1.098 ± 0.012 s,
no observable speedup. Allocations decrease from 4,189,078 to 4,168,655
(20,423 fewer, 0.49%); peak heap remains 88.88 MB. Scope boundaries now preserve
the graph, but this does not eliminate remaining expression-level normalization
or complete session-wide static solving.

Baseline `/tmp/telora-before-block-slots`; artifacts
`/tmp/block-slots-perf.{json,log}`, `/tmp/block-slots-after.zst`,
`/tmp/block-slots-after-summary.log`, `/tmp/block-slots-types-only.jsonl`.
Updated hotspot audit: `/tmp/nominal-head-normalize-callers.log`, produced from
`/tmp/nominal-head-after.zst` with `/tmp/telora-normalize-callers.awk`.

### Call results retain inference slots (2026-09-10)

Ordinary function calls no longer normalize their complete result descriptor
before returning it to the enclosing expression. They query unresolved state
and the TypeOf constructor directly, preserving the existing erasure of
incomplete TypeOf witnesses and generic-result diagnostics. Normal results
retain the callee's result slot. When no field obligations exist, call processing
also skips parameter normalization/scanning for field completion.

403 core tests and 44 CLI tests including language acceptance pass. The TypeOf
query is differentially checked against normalization before and after solving.
Release, source-size and diff checks pass; both ontology check modes pass.

Ordinary ontology `@test/query`, two warmups and five measured runs per binary,
without concurrent builds/tests/profilers: 1.101 ± 0.020 → 1.072 ± 0.016 s.
Reversed order: after 1.072 ± 0.026, before 1.094 ± 0.006 s. Both orders show
a 2–3% lower mean, but the improvement is small relative to run variability.
Allocation calls decrease from 4,168,655 to 4,053,526 (115,129 fewer, 2.76%);
peak heap decreases from 88.88 to 88.57 MB. Comparison is against the preceding
block-slot checkpoint, not the original RFC baseline.

Baseline `/tmp/telora-before-call-result`; artifacts
`/tmp/call-result-perf{,-reverse}.{json,log}`, `/tmp/call-result-after.zst`,
`/tmp/call-result-after-summary.log`, `/tmp/call-result-types-only.jsonl`.
Remaining expression/template descriptor consumers and session-wide static
phase unification remain unfinished.

### Iterative generic instantiation (2026-09-10)

Generic descriptor instantiation now uses an explicit work stack and integer
result slots, replacing recursive instantiation. The solver reuses result-buffer
capacity between calls; parameter slots and parameterized nominal-body sharing
remain independent between calls. Input templates are still descriptors, so
this does not complete arena template storage or remove all recursive adapters.

405 core tests and 44 CLI tests including language acceptance pass, as do the
release build, source-size and diff checks. New tests cover 16,384 nested array
constructors without descriptor views and nominal-body sharing within a call
while isolating separate calls.

Against the preceding call-result checkpoint, ordinary ontology `@test/query`
with two warmups and five runs per binary takes 1.045 ± 0.012 → 1.044 ± 0.013 s:
no observable speedup. Allocations decrease from 4,053,526 to 4,052,583 (943
fewer); peak heap is essentially unchanged, 88.57 → 88.58 MB. The intermediate
version allocated 5,880 more times; result-buffer reuse removes that regression.

Baseline `/tmp/telora-before-iterative-instantiation`; final artifacts
`/tmp/iterative-instantiation-buffer-perf.{json,log}`,
`/tmp/iterative-instantiation-buffer.zst`,
`/tmp/iterative-instantiation-buffer-summary.log`. Both ontology check modes
also pass. This is a stack-depth improvement, not a measured throughput gain.

### Expand contextual types only for pending joins (2026-09-10)

The final expression-inference step normalized every supplied expected type,
although that descriptor was consumed only when replacing pending alternatives
with a fully resolved contextual type. It now tests for pending alternatives
before expanding the expected graph. The existing replacement predicate and
constraint checking are unchanged; ordinary expressions retain inference slots.

405 core tests and 44 CLI tests including language acceptance pass. These include
generic common-type diagnostics and contextual construction behavior. Release,
source-size and diff checks pass, as do both ontology check modes.

Against the immediately preceding iterative-instantiation checkpoint, ordinary
ontology `@test/query`, two warmups and five measured runs per binary:
1.070 ± 0.041 → 0.975 ± 0.011 s. Reversed order: after 0.983 ± 0.003,
before 1.053 ± 0.013 s. Both orders show lower time, approximately 7–9%; the
first baseline group is noisier. Allocation calls decrease from 4,052,583 to
3,524,793 (527,790 fewer, 13.0%); peak heap changes only slightly,
88.58 → 88.38 MB. This comparison is not against the original RFC baseline.

Baseline `/tmp/telora-before-conditional-expected`; artifacts
`/tmp/conditional-expected-perf{,-reverse}.{json,log}`,
`/tmp/conditional-expected-after.zst`,
`/tmp/conditional-expected-after-summary.log`,
`/tmp/conditional-expected-types-only.jsonl`. Descriptor template ingress,
other expression consumers, and the session-wide static-first ordinary loader
remain unfinished.

### Enum members consume owner and payload slots (2026-09-10)

Expression-level enum member access now reads the constructor result, TypeOf
edge and enum body directly from inference rows. The returned member retains
the original owner and payload slots. It no longer expands unrelated variants
or function parameters. Pending alternatives still require their semantic join;
Unchecked still derives its body from its solved argument. Other descriptor
consumers, including contextual enum payload refinement, remain separate work.

407 core tests and 44 CLI tests including language acceptance pass, as do release,
source-size and diff checks and both ontology check modes. New tests verify live
payload refinement, exact owner slots and no descriptor views despite a sibling
with 16,384 nested arrays. Differential tests cover anonymous/nominal enums,
family constructors, pending alternatives, Unchecked and missing-member errors.

Against the preceding conditional-expected checkpoint, ordinary ontology
`@test/query`, two warmups and five measured runs per binary:
0.983 ± 0.016 → 0.979 ± 0.010 s. There is no observable speedup.
Allocations decrease from 3,524,793 to 3,500,193 (24,600 fewer, 0.70%);
peak heap decreases from 88.38 to 87.74 MB (0.64 MB). This is an incremental
graph-consumer migration, not completion of the session-wide phase boundary.

Baseline `/tmp/telora-before-enum-member-slots`; artifacts
`/tmp/enum-member-slots-perf.{json,log}`, `/tmp/enum-member-slots-after.zst`,
`/tmp/enum-member-slots-after-summary.log`,
`/tmp/enum-member-slots-types-only.jsonl`.

Updated streaming stack attribution records 1,327,225 allocations on stacks
containing normalization. The largest outer callers are `expose_named`
(880,547), `contextualize_authored_literal` (214,409), module analysis (84,397),
and `infer_pattern_constructors` (55,366). These are allocation counts, not CPU
shares; compiler inlining affects attribution. Remaining name exposure and
contextual literal consumers warrant the next investigation. Artifact:
`/tmp/enum-member-slots-normalize-callers.log`.

### Nominal argument refinement reads shallow shapes (2026-09-10)

Further stack attribution of the preceding profile assigns 818,237 allocations
under `expose_named` to `refine_argument_nominal_context`, out of 884,529 total
exposure allocations. Artifact: `/tmp/enum-member-slots-expose-callers.log`.
These counts measure allocations, not CPU time.

The refiner now reads each actual argument's outer constructor and original child
slots instead of normalizing the entire remaining subtree at every recursion
level. Nominal identity arguments and required alternative joins still resolve;
nominal bodies retain their shared shallow representation. Alias following and
alternative resolution use one loop so cycles crossing both steps terminate.
Parameter-side recursive reconstruction remains, as do descriptor ingress and
the unfinished session-wide static-first ordinary loader.

407 core tests and 44 CLI tests including language acceptance pass, along with
release, source-size and diff checks and both ontology check modes. Expanded
differential tests cover normalization before/after solving, nominal and
Unchecked types, aliases, alternatives, direct alias cycles and cycles through
alternatives. A 16,384-level structural input preserves exact child slots without
constructing descriptor views.

Against the preceding enum-member checkpoint, ordinary ontology `@test/query`,
two warmups and five measured runs per binary: 0.974 ± 0.009 → 0.934 ± 0.008 s.
Reversed order: after 0.929 ± 0.012, before 0.979 ± 0.012 s. Both orders show
approximately 4–5% lower time. Allocation calls decrease from 3,500,193 to
3,094,770 (405,423 fewer, 11.6%). Peak heap is essentially unchanged,
87.74 → 87.75 MB. These are incremental results, not comparisons to the original
RFC baseline or evidence that the full architecture migration is complete.

Baseline `/tmp/telora-before-refinement-shapes`; artifacts
`/tmp/refinement-shapes-perf{,-reverse}.{json,log}`,
`/tmp/refinement-shapes-after.zst`, `/tmp/refinement-shapes-after-summary.log`,
`/tmp/refinement-shapes-types-only.jsonl`.

### Nominal refinement retains parent edges (2026-09-10)

After refining a known parameter slot, the refiner now returns that original
slot instead of its reconstructed descriptor. Parents already reference this
slot and observe its refinement directly, so a child change no longer creates
new type rows at every ancestor. The updated descriptor is moved into the slot
without an extra clone. Local shallow descriptor construction and recursive
traversal remain; this is not yet a wholly slot-based iterative refiner.

408 core tests and 44 CLI tests including language acceptance pass, along with
release, source-size and diff checks and both ontology check modes. A regression
test verifies that nominal refinement updates a child while preserving its parent
row and edge, leaves an independent parameter graph unchanged, and adds no rows
when the same refinement is repeated.

Against the preceding shallow-refinement checkpoint, ordinary ontology
`@test/query`, two warmups and five measured runs per binary:
0.940 ± 0.011 → 0.924 ± 0.024 s. Reversed order: after 0.902 ± 0.012,
before 0.937 ± 0.017 s. Both orders have lower means, but the first comparison
is small relative to variability; treat timing as an improvement trend rather
than a stable percentage. Allocation calls decrease from 3,094,770 to 2,926,097
(168,673 fewer, 5.45%). Peak heap is essentially unchanged, 87.75 → 87.59 MB.
These results are relative to the preceding checkpoint, not the original RFC
baseline. The ordinary loader's session-wide static-first boundary remains open.

Baseline `/tmp/telora-before-refinement-slots`; artifacts
`/tmp/refinement-slots-perf{,-reverse}.{json,log}`,
`/tmp/refinement-slots-after.zst`, `/tmp/refinement-slots-after-summary.log`,
`/tmp/refinement-slots-types-only.jsonl`.

### Native types enter through static contracts (2026-09-10)

The builtin inventory now supplies native types through explicit interfaces.
Ordinary analysis reads the concrete opaque contract instead of decoding the
linked native heap value. Missing/invalid contracts fail explicitly, and the
unused evaluator descriptor-decoding method has been removed. See the
[handoff audit](static-phase-handoff.md) for the remaining phase dependencies.

409 core tests and 44 CLI tests including language acceptance pass. After removal
of the unused decoder, the contract test and final release build pass without
the new dead-code warning. Source-size/diff checks and both ontology check modes
also pass. This checkpoint changes the native descriptor source, not the
ordinary loader's execution ordering. No new throughput or allocation claim is
made; the previous measured performance checkpoint remains the reference.

Preserved binary `/tmp/telora-before-native-contracts`; verification artifacts
`/tmp/native-contract-core.log`, `/tmp/native-contract-cli.log`,
`/tmp/native-contract-focused-final.log`, `/tmp/native-contract-release-final.log`,
`/tmp/native-contract-check.jsonl`, `/tmp/native-contract-types-only.jsonl`.

### Ordinary imports use interface facts (2026-09-10)

Ordinary import type selection and the pure checker now use one static interface
reader. It distinguishes namespace and selected value through `value_binding`,
including empty namespaces, and rejects selected bindings without a contract.
It does not accept a runtime value. The old empty-interface runtime-kind test is
removed; no-interface Host inputs and recovery value-fact projection remain
separate, unfinished migration work.

410 core tests, 44 CLI tests including language acceptance, release and
source-size/diff checks pass; both ontology check modes pass. A new test checks
empty/populated namespaces, selected values and malformed selected contracts
without runtime resources. No new timing/allocation claim is made for this
architecture checkpoint.

Preserved binary `/tmp/telora-before-import-contracts`; artifacts
`/tmp/import-contract-core.log`, `/tmp/import-contract-cli.log`,
`/tmp/import-contract-release.log`, `/tmp/import-contract-check.jsonl`,
`/tmp/import-contract-types-only.jsonl`.

### DataWorld carries source-derived contracts (2026-09-10)

Primitive Host factories now supply an explicit contract. JSON/TOML/YAML derive
their contracts from the validated source plan before runtime materialization,
using a postorder worklist and canonical type graph IDs for structural equality.
The external interface still consumes a final descriptor. Heap and contract share
one HostData owner, so cloning DataWorld does not clone the contract tree.

Strict DataWorld analysis and module loading pass the contract as an interface.
The DataWorld partial-analysis wrapper no longer creates a heap, publishes values,
or derives types from values. Direct PersistentValue-based observed/recovery
adapters remain unfinished; this is not yet complete Host boundary migration.

Final verification: 412 core tests, 44 CLI tests including language acceptance,
release, source-size/diff checks and both ontology check modes pass. New tests
cover shared/forward plan edges, homogeneous/mixed/empty arrays, untyped tags,
and differential agreement with existing data-value shapes. Existing metadata
boundary tests still prevent Host metadata from impersonating type declarations.
No timing or allocation improvement is claimed for this architecture change.

Preserved binary `/tmp/telora-before-host-contracts`; final artifacts
`/tmp/host-contract-core-final.log`, `/tmp/host-contract-cli-final.log`,
`/tmp/host-contract-focused-final.log`, `/tmp/host-contract-release-final.log`,
`/tmp/host-contract-check-final.jsonl`, `/tmp/host-contract-types-only-final.jsonl`.

### Remove remaining runtime-value type inference (2026-09-10)

External and recovery type inputs now use explicit interface contracts, including
hidden property roots. Deleted the generic recursive value-inference fallback.
Final validation: 412 core tests, 44 CLI tests including language acceptance,
release build and ontology types-only check pass.

Immediate baseline `/tmp/telora-before-no-value-fallback` versus current release,
ontology `check @test/query`, hyperfine 2 warmups and 5 measured runs:
897.9 ± 8.7 ms versus 906.4 ± 9.6 ms. No speedup is established. Heaptrack records
2,926,205 versus 2,925,925 allocations and 87.58 versus 87.59 MB peak heap;
these are effectively unchanged. This comparison is against the immediately
preceding contract checkpoint, not the original performance baseline.

A separate current-release comparison (2 warmups, 5 runs) gives:

| Mode | Mean ± standard deviation |
| --- | --- |
| `check --types-only @test/query` | 600.6 ± 2.8 ms |
| `check @test/query` | 919.9 ± 19.0 ms |

The approximately 319 ms difference is between separate checking paths, not
an exact measurement of metadata/runtime phases. JSON summaries expose
`catalog_seconds` and `check_seconds`; precise stage accounting remains gated
on a shared static artifact and driver.

Artifacts: `/tmp/no-value-fallback-perf.{log,json}`,
`/tmp/no-value-fallback-{before,after}-summary.log`,
`/tmp/types-only-current-perf.{log,json}`,
`/tmp/no-value-fallback-types-only.jsonl`.

### Defer evaluator acquisition until module static plans are solved (2026-09-10)

Removed the two remaining runtime Module-kind probes from ordinary interface
publication. Evaluator/bootstrap/pending construction-check setup now follows
static plan preparation. This is not yet session-wide static-first execution.

Immediate baseline `/tmp/telora-before-evaluator-deferral`, same ontology workload,
hyperfine 2 warmups/5 runs: baseline 953.3 ± 19.4 ms, current 912.7 ± 11.2 ms.
Reversed order: current 929.6 ± 14.2 ms, baseline 936.4 ± 20.9 ms. The reverse
comparison does not confirm the initial 4% difference; no stable speedup is
claimed. Allocation impact was not measured for this checkpoint.

Validation: existing 412 core tests passed; the new static-rejection main-heap
test passed after correcting its expected diagnostic text. All 44 CLI integration
tests (including language acceptance), release build, ontology types-only check,
source-size and diff checks passed. The main-heap test does not prove absence of
temporary work-heap allocation; evaluator placement is the source-level evidence
for the new boundary, which still requires extraction into a VM-free API.

Artifacts: `/tmp/evaluator-deferral-{core-final,focused,cli,release}.log`,
`/tmp/evaluator-deferral-perf{,-reverse}.{log,json}`,
`/tmp/evaluator-deferral-types-only.jsonl`.

### Defer external runtime linking (2026-09-10)

Removed runtime-value registration from the static binding traversals. One
post-solving link step handles external/import/native values and missing dynamic
inputs; import/native type selection remains based on mandatory static contracts.
This removes repeated import/native registrations and the obsolete early import
pass. It is an architectural checkpoint, with no new timing/allocation claim.

413 core tests pass. Final CLI validation passes 22 library, 2 binary and 44
integration tests including language acceptance; release build and both ontology
check modes pass. Source-size and diff checks pass with existing review warnings.
Artifacts: `/tmp/deferred-links-{core,cli,release}.log`,
`/tmp/deferred-links-{check,types-only}.jsonl`.

### Static owner evidence and graph-root materialization (2026-09-10)

Owner capture evidence and parameter substitution now run before execution;
runtime consumes plans containing graph IDs, link names and arities. Owner
metadata uses one batch through the shared graph materialization table instead
of per-owner descriptor materialization. Runtime type evidence normalization
also moves before evaluator acquisition. Interface finalization and runtime
family handles remain unfinished handoff work.

Immediate baseline `/tmp/telora-before-owner-plans`, ontology ordinary check,
hyperfine 2 warmups/5 runs: 946.7 ± 47.6 ms versus 910.8 ± 13.6 ms. Baseline
variance is large; no stable throughput improvement is claimed. Heaptrack:
2,925,979 → 2,920,574 allocations (-5,405, about 0.18%); temporary allocations
170,548 → 176,131; peak heap 87.57 → 87.60 MB. No peak-memory reduction.

413 core tests, 44 CLI integration tests including language acceptance, release,
ontology types-only and source-size/diff checks pass. Ordinary ontology check
also passes in all benchmark/profile runs. No inference fallback was added.

Artifacts: `/tmp/owner-plan-core-final.log`, `/tmp/owner-plan-{cli,release}.log`,
`/tmp/owner-plan-perf.{log,json}`, `/tmp/owner-plan-{before,after}-summary.log`,
`/tmp/owner-plan-types-only.jsonl`.

### Static family interfaces and inference release before execution (2026-09-10)

ModuleInterface now carries static nominal family constructor identities instead
of runtime templates. Import/re-export paths consume these identities; runtime
families remain execution/compiler links. The duplicated template table and
unused parameter copies are removed. Interface and compiler evidence now finish
before execution, followed by an explicit `drop(inference)`.

Immediate baseline `/tmp/telora-before-static-family-interface`, ontology ordinary
check, 2 warmups/5 runs: baseline 912.8 ± 15.7 ms, current 942.1 ± 38.6 ms
(outlier warning). Reverse order: current 929.4 ± 10.4 ms, baseline 920.0 ± 14.4 ms.
No throughput improvement; the small slower tendency remains within the observed
variation and should not be represented as a speedup.

Heaptrack: 2,920,623 → 2,917,965 allocations (-2,658, about 0.09%); peak heap
87.60 → 86.20 MB (-1.40 MB, about 1.6%). Temporary allocations 176,170 → 176,171.
This compares only the immediate preceding owner-plan checkpoint.

413 core tests, 44 CLI integration tests including language acceptance, release,
ontology types-only and source-size/diff checks pass. Ordinary ontology checks
pass in all benchmark/profile runs. Runtime template field search confirms no
remaining `type_family_templates` interface path. The encompassing analysis API
and module driver still require session-level separation; this is not completion
of the overall static-first architecture.

Artifacts: `/tmp/static-family-interface-{core,cli,release}.log`,
`/tmp/static-family-interface-perf{,-reverse}.{log,json}`,
`/tmp/static-family-interface-{before,after}-summary.log`,
`/tmp/static-family-interface-types-only.jsonl`.

### Extract VM-free module solver and solved artifact (2026-09-10)

Ordinary analysis now hands a SolvedModulePlan from `solve_module_plan` to
`execute_module_plan`. The solver accepts no VM/heap/runtime roots; the artifact
owns the graph and evidence, borrowing only source syntax for prepared plans.
Solved tool bindings are distinct from execution tasks containing value slots.
No graph clone or second inference pass is introduced by the handoff.

414 core tests pass, including a direct no-heap solver test with an exported
generic nominal family and `1 / 0`. 44 CLI integration tests including language
acceptance, release, ontology types-only and source-size/diff checks pass.
Ordinary ontology checking passes throughout benchmarking.

Immediate baseline `/tmp/telora-before-solved-module`, 2 warmups/5 runs:
937.2 ± 11.9 → 921.6 ± 8.9 ms. This small single-order difference is not a claim
of stable throughput gain. Allocation/peak-memory impact was not remeasured.
The driver remains module-at-a-time, and the separate pure loader has not yet
been migrated to consume this same artifact.

Artifacts: `/tmp/solved-module-core-final.log`, `/tmp/solved-module-{cli,release}.log`,
`/tmp/solved-module-perf.{log,json}`, `/tmp/solved-module-types-only.jsonl`.

### Types-only and ordinary checking share the static solver (2026-09-10)

Removed the standalone full types-only checking implementation and its separate
decorator checker. The adapter now invokes solve_module_plan, moves its import
interface map, and shares a TypeStore across the static module workspace.
The modes still have different loaders/publication paths.

Immediate baseline `/tmp/telora-before-shared-static`, ontology query, hyperfine
2 warmups/5 runs:

| Path | Mean ± standard deviation |
| --- | --- |
| Previous types-only | 596.9 ± 6.6 ms |
| Shared-solver types-only | 737.6 ± 9.7 ms |
| Current ordinary check | 920.1 ± 19.3 ms |

Types-only regresses by about 23.6%. The shared path now prepares ordinary
execution plans and compiler evidence too; the measurement does not isolate
which substage accounts for the additional time. Keep one solver and separate
typed evidence from code-generation preparation next, rather than restoring the
old checker. The approximately 183 ms current mode difference is still not exact
metadata/runtime phase accounting.

Types-only heaptrack: allocations 2,347,937 → 2,288,341 (-59,596, about 2.54%);
peak heap 88.04 → 83.28 MB (-4.76 MB, about 5.41%); temporary allocations
131,674 → 123,691. These compare types-only to types-only, not ordinary check.

414 core tests and 44 CLI integration tests including language acceptance pass.
After the final interface-ownership change, core static-check and CLI types-only
focused tests, cargo check and a fresh release build pass. Both ontology modes
pass in benchmark/profile runs; source-size/diff checks pass with existing
review warnings.

Artifacts: `/tmp/shared-static-{core,cli}.log`,
`/tmp/shared-static-focused-{core,cli}.log`, `/tmp/shared-static-release-final.log`,
`/tmp/shared-static-perf.{log,json}`, `/tmp/shared-static-{before,after}-summary.log`.

### Defer tool bytecode generation to execution (2026-09-10)

Static preparation retains lowered expressions, solved owner/constructor evidence
and validated external links. Compiler/LIR/bytecode generation now occurs on the
execution consumer's first use and its result is reused. The code-generation
API receives no inference context. Type checking, name validation, elaboration
and witness preparation remain static.

Immediate baseline `/tmp/telora-before-deferred-tool-codegen`, ontology query,
2 warmups/5 runs: types-only 718.9 ± 6.6 → 739.6 ± 28.0 ms (outlier warning).
Reverse order: current 731.4 ± 16.2 ms, baseline 728.7 ± 6.0 ms. No measurable
speedup. Ordinary check in the first batch: 917.6 ± 29.3 → 905.0 ± 14.4 ms;
no stable ordinary-check improvement is established either.

Types-only heaptrack: 2,288,277 → 2,287,947 allocations (-330); temporary
allocations 123,649 → 123,643; peak heap unchanged at 83.28 MB. Tool bytecode
generation therefore does not explain the previous shared-solver cost increase.
Next investigate static publication/canonicalization costs rather than attributing
that regression to code generation without evidence.

414 core tests, the extended lazy-codegen/repeated-execution regression test,
44 CLI integration tests including language acceptance, release and source-size/
diff checks pass. Both ontology modes pass in benchmark/profile runs. Prepared
syntax is still retained with generated code; no memory saving is claimed.

Artifacts: `/tmp/deferred-tool-codegen-{core,focused,cli,release}.log`,
`/tmp/deferred-tool-codegen-perf{,-reverse}.{log,json}`,
`/tmp/deferred-tool-codegen-{before,after}-summary.log`.

### CPU profile and early imported-body lookup (2026-09-10)

Software perf works: cpu-clock:u at 499 Hz with DWARF call graphs. Initial
types-only recording has 326 samples, with contains_type_variable at 18.7%
self time. Moved the existing imported-body lookup before recursive eligibility
scans in import_declared_body; unresolved arena edges remain shared.

Immediate baseline `/tmp/telora-before-body-ingress-fastpath`, 2 warmups/5 runs:
types-only 734.1 ± 20.7 → 722.9 ± 9.0 ms; ordinary check 907.6 ± 11.8 →
915.1 ± 7.0 ms. No stable improvement is established. A subsequent short profile
has 347 samples and still attributes 17.0% self time to contains_type_variable.
These short samples identify a continuing hotspot, not a precise reduction.

414 core tests, 44 CLI integration tests including language acceptance, release,
both ontology modes and source-size/diff checks pass. Existing tests cover
shared unresolved body edges and distinct recursive view completeness.
Artifacts: `/tmp/static-solver-current.perf`, `/tmp/static-solver-perf-report.log`,
`/tmp/body-ingress-{core,cli,release}.log`, `/tmp/body-ingress-perf.{log,json}`,
`/tmp/body-ingress-current.perf`, `/tmp/body-ingress-report.log`.

### Reuse finalized binding descriptors across publication (2026-09-10)

After solving, binding-first ID reservation now retains its normalized descriptor
map. Owner/unresolved validation, exported monomorphic schemes, interface output
and final graph-ID output reuse that map instead of independently normalizing
each binding. The map moves into interface_binding_types; no extra full map copy
or skipped validation is introduced.

Immediate baseline `/tmp/telora-before-binding-publication` includes the prior
imported-body lookup change. Ontology query, hyperfine 2 warmups/5 runs:

| Mode | First comparison, baseline → current | Reverse comparison, baseline → current |
| --- | --- | --- |
| types-only | 742.9 ± 8.1 → 710.7 ± 12.1 ms | 732.8 ± 8.7 → 705.4 ± 10.4 ms |
| ordinary check | 942.8 ± 35.1 → 910.5 ± 35.9 ms | 905.4 ± 8.5 → 883.5 ± 6.7 ms |

Both orders support roughly 4% less types-only time; ordinary checking also
improves, with the less noisy reverse comparison showing about 2.4%.
Types-only heaptrack: 2,287,947 → 2,262,641 allocations (-25,306, about 1.1%);
temporary allocations 123,648 → 123,647; peak heap 83.28 → 83.27 MB (effectively
unchanged). These are incremental results, not original-baseline comparisons.

414 core tests, 44 CLI integration tests including language acceptance, release,
both ontology modes and source-size/diff checks pass. Artifacts:
`/tmp/binding-publication-{core,cli,release}.log`,
`/tmp/binding-publication-perf{,-reverse}.{log,json}`,
`/tmp/binding-publication-{before,after}-summary.log`.
