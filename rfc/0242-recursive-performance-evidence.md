# RFC 0242: Recursive Performance Evidence

- Status: Proposed
- Tracking issue: #83
- Depends on: RFC 0241

## Summary

Telora will turn the reproductions under
`crates/telora/tests/fixtures/performance/type-structure` into a stable
performance protocol and targeted correctness regressions. Performance claims
will identify the command, build profile, fixture, repeated samples, and median
user time. Tests will assert semantic boundaries; elapsed-time ceilings are
reserved for coarse exponential-regression guards.

## Protocol

The repository provides one command that:

1. requires an existing release `telora` binary rather than timing compilation;
2. runs flat functions, recursive functions, nested functions, shallow
   recursive values, growing recursive values, and QueryBuilder independently;
3. performs a warm-up followed by at least three measured runs;
4. reports JSON Lines containing fixture, operation, sample count, median wall
   time, and median user time; and
5. exits nonzero if a command fails.

The protocol is diagnostic, not a portable microbenchmark score. Absolute
times vary by host; comparisons use the same binary, host, and load conditions.

## Correctness regressions

Focused Rust tests establish that:

- matching declared values validate successfully;
- a different declared owner fails even when its body has equal structure;
- a raw value with an invalid nested field fails before identity acquisition;
- a valid raw value may be validated and branded exactly once;
- Work/Main copying retains shared handles; and
- legacy projection rejects cycles.

The growing fixture also receives a coarse release-only or ignored regression
check suitable for explicit performance runs. Normal unit tests must not become
host-speed-sensitive.

## Instrumentation

Implementation may add test-only counters around descriptor comparison, value
validation, graph copying, and legacy projection. Counters are evidence, not
language or CLI surface, and must not impose synchronization cost on normal
builds. If phase timing is added to the CLI, it must be an explicitly enabled
machine-readable diagnostic and must not alter ordinary stdout.

## Rejected alternatives

- A single timing sample is too sensitive to load and cache state.
- A strict subsecond assertion in ordinary CI is not portable.
- Profiling only QueryBuilder cannot separate descriptor and value-graph costs.
- Reducing the recursive fixture depth conceals the defect.

## Acceptance criteria

1. the performance protocol is documented and executable from the repository
   root;
2. output is machine-readable JSONL and includes median user time;
3. all six issue fixtures are measured separately;
4. correctness tests cover trusted and untrusted declared-value boundaries;
5. the protocol records a baseline before RFC 0243 implementation; and
6. normal workspace tests remain deterministic.

