# RFC 0278: Result-Based Construction Checks

- Status: Accepted; implementation pending.
- Tracking: [#172](https://github.com/hh9527/telora/issues/172).
- Supersedes: the check return protocol of [RFC 0275](0275-construction-checks-and-unchecked.md).
- Depends on: [RFC 0277](0277-tuple-types-unit-and-type-metadata.md).

## Motivation and Scope

Construction checks validate a candidate without producing replacement data.
With an explicit Unit type and value, their natural result is
`Result((), BlameError)`: success without data, or a structured rejection.
The previous `Option(BlameError)` convention represented success as None and
rejection as Some, opposite to the success/failure direction of Option's `?`.
Result allows ordinary validation helpers to compose with `?`.

This is an intentional breaking change to the check protocol, not a general
replacement of Option and not a performance optimization.

## Semantics

| Declaration | Check contract |
| --- | --- |
| `type T = struct { ... };` | `Fn(Unchecked(T)) -> Result((), BlameError)` |
| `type T = struct(U);` | `Fn(U) -> Result((), BlameError)` |
| `type T = enum { A(U) };`, on A | `Fn(U) -> Result((), BlameError)` |
| Unit variant | No check; direct construction |

`Unit` remains an alias for `()`, so `Result(Unit, BlameError)` is equivalent.
Checks return `Ok(())` to accept the original candidate, or `Err(error)` to
reject it. They cannot replace, transform or return the candidate on success.
There is no implicit lifting from Unit to Result: an empty or semicolon-ended
body alone is not a successful checker. Never-returning branches retain the
existing directional checking rules.

A Result propagation boundary with a Never tail can still return an earlier
Err through `?`. Its inferred result is `Result(Never, E)`, directionally
compatible with the expected `Result((), BlameError)` contract. Do not reject
such a checker merely because its normal success tail cannot return.

```telora
import "std/blame" {BlameError};

def nonnegative: Fn(Int) -> Result((), BlameError) = fn(value) {
    if value >= 0 { Ok(()) }
    else { Err(blame!("nonnegative required", value)) }
};

@check(fn(value) {
    nonnegative(value.min)?;
    nonnegative(value.max)?;
    if value.min <= value.max { Ok(()) }
    else { Err(blame!("invalid range", value.min, value.max)) }
})
type Range = struct {min: Int, max: Int};
```

All existing construction paths use this protocol: direct and first-class
constructors, unchecked conversion, generic and recursive construction,
merge-update, projection, checked casts, codec decoding, parsing, and tool-stage
evaluation. Ordinary construction raises a returned error at the construction
boundary. Codec decoding returns it as `Err(BlameError)`; untagged trials may
reject a candidate without emitting a diagnostic. Checker execution failure
(including explicit raise/fail or quota exhaustion) remains distinct from a
normal rejection and is not swallowed as an alternative mismatch.

Preserve original candidate values, error subjects and provenance, canonical
nominal identities, and check timing. Reading, copying, encoding or casting an
already checked value must not repeat its check. Existing metadata publication,
dependency scheduling and global inference remain unchanged.

`warn!(error)` remains an independently useful operation returning None with
its existing Option contract. A warning-only checker now writes
`let warning: Option(()) = warn!(error); Ok(())`. The annotation supplies the
otherwise unconstrained Option item type; merely discarding warn!'s result
does not provide that context. Neither warnings nor ordinary Option APIs change here.

## Alternatives

- Keep Option: rejected because it reverses propagation and no longer fills a
  gap in the type system.
- Accept both protocols: rejected; one exact contract provides consistent
  diagnostics and composition without permanent compatibility paths.
- Implicitly lift Unit or return `Result(T, BlameError)`: rejected; checks
  explicitly validate and do not perform conversions.
- Change warn! to Unit: deferred as a separate public intrinsic change.

## Implementation Plan

1. Commit this RFC before implementation.
2. Replace the static expected return descriptor and audit every runtime
   consumer of construction check results. Validate the Ok Unit payload, and
   retain the original error payload on rejection.
3. Migrate active language fixtures, Rust-embedded programs, performance
   fixtures, examples and guides. Preserve historical RFC text except for a
   supersession notice on RFC 0275.
4. Add focused positive and negative language cases and runtime boundary
   coverage. Run workspace tests, release build and source hygiene checks.
5. Record evidence here and in #172, commit and push, then close #172.

## Executable Acceptance Criteria

- Inferred and annotated checks accept `Ok(())` / `Err(BlameError)`;
  `Result(Unit, BlameError)` is accepted as the same contract.
- Multiple Result-based validation helpers compose through `?`, stopping at
  the first error and preserving its original subjects. A Never tail after
  `?` preserves both early rejection and execution failure on the success path.
- Legacy None/Some, plain Unit (including fallthrough), non-Unit Ok payloads,
  wrong error types and wrong parameter types are rejected statically.
- Migrated construction, generic/import/recursive, merge/projection, cast,
  codec/untagged, parse and tool-stage suites pass. Ordinary rejection and
  checker execution failure remain distinct; provenance tests remain green.
- Warning-only checks explicitly return `Ok(())`; invocation-count tests still
  prove no repeated validation of completed values.
- Runtime consumers reject malformed check results rather than accepting any
  Ok-tagged value. Success publishes the original candidate, not Unit.
- `cargo test --workspace`, `cargo build --release`, `git diff --check` and
  `scripts/check-source-size.sh` pass. Report any pre-existing formatting issues
  separately without unrelated formatting churn.
- Implementation is committed and pushed before the tracking issue is closed.
