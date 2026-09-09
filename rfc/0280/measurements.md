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
