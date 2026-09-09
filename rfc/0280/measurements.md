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
