# RFC 0279: Unified Result Diagnostics

- Status: Accepted; implementation pending.
- Tracking: [#174](https://github.com/hh9527/telora/issues/174).
- Depends on: RFC 0275, RFC 0277 and RFC 0278.
- Supersedes: the function-specific diagnostic macros of RFC 0101/0189/0231,
  the warning Option return of RFC 0275, and the warning workaround of RFC 0278.

## Motivation and Scope

Separate ordinary calls, Result inspection, error construction and diagnostic
emission. Preserve two convenient contextual operations without giving them
independent error semantics:

```telora
func(a, b)                 // Result, handled explicitly by the caller
func(a, b).unwrap!()       // success payload, or raise the error
func(a, b).ok_or_warn!()   // Some(payload), or warn and return None
```

Remove `should_ok!`, `must_ok!`, `try_unwrap!` and the native
`std/result.unwrap` entry. This is a breaking migration without compatibility
aliases. Ordinary `test.should_ok(...)` test constructors remain unchanged.

## Error Boundary

`raise!(error)` and `warn!(error)` share exactly one normalization rule:

| Input | Message | Data references | Rule |
| --- | --- | --- | --- |
| String | String contents | Empty | Authored intrinsic call site |
| BlameError | Stored message | Stored explicit subjects | Authored intrinsic call site |

The String value's location is not a data reference. Neither Result containers
nor function arguments become implicit diagnostic subjects. BlameError retains
the subjects and their original locations; no recursive object inspection or
display-based error conversion is introduced. Existing diagnostic serialization
and unavailable-location behavior are preserved.

Only String and the canonical opaque BlameError are supported (plus Never's
ordinary bottom compatibility). This is a closed intrinsic overload, not Any,
a union type, a new public trait, or a generic display contract. Unresolved
error types need type evidence; unconstrained `for(E)` cannot promise support
for arbitrary E. Invalid concrete error types are rejected statically.

`raise!` emits failure and returns Never. `warn!` emits a warning, continues,
and returns `()`. Reporting a warning no longer invents an Option item type.
The call-site rule for these operations is their own authored invocation,
including inside helper functions; a wrapper's outer caller must not replace
the explicit emission site. Existing stack/implementation traces remain
available separately.

## Result Operations

These definitions specify semantics, not user-defined macro syntax:

```telora
unwrap!(result) = match result {
    Ok(value) => value,
    Err(error) => raise!(error),
}

ok_or_warn!(result) = match result {
    Ok(value) => Some(value),
    Err(error) => { warn!(error); None },
}
```

They accept Result(T, E), with E checked using the same error-boundary rules.
They return T and Option(T), respectively. An operand, its callee, and each
argument are evaluated exactly once in ordinary left-to-right order. Generated
bindings are hygienic; prefix and postfix forms are equivalent. The generated
raise/warn uses the user's unwrap/ok_or_warn invocation location, never a
standard-library implementation location or the source of the Result value.
Existing `?` continues to propagate values without emitting diagnostics.

## Other Intrinsics

`blame!(message, subjects...)` remains a non-emitting opaque error constructor.
`fail!(message, subjects...)` remains strict semantic sugar for
`raise!(blame!(message, subjects...))` at the fail invocation site. A fused
implementation may avoid allocating a temporary error object. It must not
change message, subjects or failure behavior relative to that composition.

`panic!` retains its implementation-error category. `dbg!`, `ty!` and `cast!`
are unchanged. In particular, this RFC does not redefine cast's shape mismatch
and nominal construction rejection behavior. Reserved file!/line! remain
unimplemented. Silent Result-to-Option helpers are not required by this RFC.

## Migration

- `f.must_ok!(args...)` becomes `f(args...).unwrap!()`.
- `f.should_ok!(args...)` becomes `f(args...).ok_or_warn!()`.
- `result.try_unwrap!()` becomes `result.ok_or_warn!()`.
- `std/result.unwrap(result)` becomes `result.unwrap!()`; pipeline uses become
  `|> unwrap!(_)`. Remove imports made unused by that migration.
- Code relying on automatic argument evidence must construct
  `Err(blame!(message, subjects...))` explicitly; do not add implicit evidence
  back to the new operations to preserve old fixtures.
- `warn!` callers use Unit or a statement. Warning-only checks write
  `warn!(blame!(message, subject)); Ok(())`.

## Implementation Plan

1. Commit this RFC before implementation and record it on #174.
2. Replace parser expansion with one Result operand and shared raise/warn
   operations. Remove obsolete native unwrap and private warning bypasses.
3. Implement the closed error input checks, Unit warning result, shared runtime
   normalization and invocation-site location policy, including fail sugar.
4. Migrate stdlib, examples, active guides/design text and test programs.
   Historical RFCs get supersession notices instead of rewritten histories.
5. Add focused language and Rust regression coverage, run full workspace tests,
   release build and source hygiene checks. Record evidence, commit and push,
   then close #174 only after remote delivery is verified.

## Acceptance

- String and BlameError work in both prefix/postfix Result macros and direct
  raise/warn. Result matching and test.should_ok still work.
- Success returns the original value in T/Some(T); warning failure returns None;
  direct warnings have Unit type and work as standalone statements.
- Direct and generated emission share messages, error category and explicit
  sources. String errors have no subject labels; BlameError's cross-module
  subject locations survive. Result allocation and String origin do not leak
  into subject labels.
- Rule positions identify the user macro call, including inside nested/imported
  helpers; diagnostic traces do not substitute outer calls as the rule.
- Callee, argument and Result operand evaluation is once-only and ordered;
  successful unwraps produce no diagnostic. Explicit fail composition matches
  raise(blame) semantics, while panic remains a distinct failure category.
- Invalid error types and all removed entry points fail statically. Public
  signatures must not claim arbitrary E support and defer rejection to runtime.
- Existing source-provenance, resource-limit, best-effort and construction tests
  pass with explicit evidence migrations where required.
- `cargo test --workspace`, `cargo build --release`, `git diff --check` and
  `scripts/check-source-size.sh` pass. Report pre-existing full-formatting
  differences separately, without unrelated formatting churn.
