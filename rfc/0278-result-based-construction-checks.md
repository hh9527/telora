# RFC 0278: Result-Based Construction Checks

- Status: Implemented and verified.
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

## Implementation and Acceptance Evidence

The RFC was committed before implementation as `873a7b0`; propagation and warning
context clarifications were committed as `f2ecd1b` before the inference fix.
The static check contract now uses the existing Result descriptor with an empty
Tuple success type. The shared construction continuation accepts only Ok with
an empty Tuple payload, or Err with the canonical opaque BlameError payload.
It still returns the original candidate to ordinary callers and wraps that
candidate, not Unit, for codec callers. Error objects are retained unchanged.

The Result propagation boundary now preserves earlier error returns when its
tail is Never, using `Result(Never, E)`. No parser, Option protocol, property
scheduler, environment-copying or type-identity changes were needed. Warning
checks provide an explicit `Option(())` binding before returning `Ok(())`.

| Requirement | Executable evidence |
| --- | --- |
| Inferred and annotated checks, Unit alias, composed helpers, short-circuiting, Never tail | `test/check-result` (9 cases) |
| Cross-module propagated error subject location | `test/check-result-provenance` |
| Legacy None/Some, Unit, empty/semicolon bodies, wrong Ok/Err payloads, warning tail rejected | Eight new `check/diag-check-*` fixtures |
| Wrong input, duplicate check, invalid field and unit-variant targets | Existing `check/diag-check-*` fixtures migrated to Result |
| Direct/generic/imported/recursive construction, merge and projection | `test/construction-check`, `test/construction-boundaries`, `test/checked-recursive-types` |
| Codec nested/untagged rejection vs execution failure, checked casts, parsing and provenance | `test/codec-construction-check`, `test/cast-construction-check`, `test/parse-construction-check`, `test/parse-check-provenance` |
| Actual tool-stage construction from decorator arguments | `check/check-result-tool-stage`, `check/diag-check-result-tool-stage` |
| Forward check dependencies | `check/check-tool-dependencies` |
| No repeated checks for completed values | `test/construction-check-once`: 1 warning for copies, 3 for a two-update chain |
| Malformed runtime results, original candidate and error object retention | Three `vm::tests::construction_result_*` tests |
| Result propagation with Never tails and incompatible error rejection | `types::tests::result_propagation_keeps_error_returns_before_never_tails` |
| Recursive fuel/stack/allocation/call-depth limits | `module::tests::recursive_construction_and_codec_trials_preserve_resource_limits` |

The two older `diag-check-early-construction` / `diag-check-tool-construction`
fixtures now reject computed metadata as types under RFC 0277. They were migrated
but are not counted as tool-stage execution evidence; the new decorator-argument
fixtures above explicitly cover actual execution.

Verification on 2026-09-09:

- `cargo test --workspace`: passed, including 322 core tests, 41 CLI tests,
  all 389 language fixture groups, and remaining workspace/doc tests.
  Log: `/tmp/rfc0278-workspace-complete.log`.
- `cargo build --release`: passed. Log: `/tmp/rfc0278-release.log`.
  Release smoke tests passed for `check-result/testee` (9 cases) and
  `codec-construction-check/testee` (11 cases), using the isolated fixture copy
  `/tmp/telora-rfc0278-wvfoIT`.
- `git diff --check`, staged diff checking and `scripts/check-source-size.sh`:
  passed. The size script still reports three existing review-only large files.
- New Rust test files and the static construction contract pass targeted
  rustfmt checks. Repository-wide `cargo fmt --all --check` still reports
  existing formatting differences, including untouched `source_arg.rs`,
  `ast.rs` and `bytecode.rs`; unrelated formatting was left unchanged.
- Active guide/design text and the runtime performance fixture are migrated.
  Remaining legacy None/Some check examples in active fixtures are intentional
  negative tests. Historical RFC 0275 retains its original text with an explicit
  supersession notice. No tree-sitter submodule change is required.
