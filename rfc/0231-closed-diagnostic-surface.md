# RFC 0231: Closed diagnostic surface

- Status: Implemented
- Tracking issue: #59
- Replaces the public surfaces of RFCs 0105, 0188, and 0189

## Summary

Telora exposes exactly seven contextual diagnostic intrinsics:

```telora
dbg!(value)
dbg!(value, "message")
should_ok!(checker, arguments...)
must_ok!(checker, arguments...)
try_unwrap!(result)
unwrap!(result)
fail!(message, subjects...)
panic!(message)
```

The list contains seven intrinsic names; `dbg!` has two arities. No compatibility
aliases are retained for `blame!`, `raise!`, `emit_info!`, `emit_warn!`, or
`emit_error!`. Ordinary Telora code cannot import or call `report`, construct a
`BlameError`, or name `Severity` or `BlameError` in a contract.

The implementation keeps an internal blame envelope, dedicated warning
operation, and raise instruction. They carry authored rule provenance, subject
provenance, warning observation, and failure control flow between the evaluator
and Host; they are not MainWorld/WorkWorld Telora values or public language
vocabulary. There is no
generic report operation or severity enum.

## Motivation

The current surface exposes both mechanism and policy:

```telora
let error = blame!(message, value);
let ignored = emit_error!(message, value);
raise!(error)
```

This admits states whose authority is unclear: an error can be constructed but
unused, reported as Error while ordinary control flow still returns success,
or stored in a domain `Result` even though it is an evaluator-to-Host failure
envelope. Programs must understand provenance plumbing to express the common
facts that a result remains valid, cannot be produced, or an invariant is
broken.

The closed surface separates these meanings:

| Surface | Meaning | Result |
| --- | --- | --- |
| `dbg!` | observe an explicitly valid value during development | the same `A` |
| `should_ok!` | turn a typed recoverable rejection into warning plus filtering | `Option(R)` |
| `must_ok!` | turn a typed rejection into failure | `R` or `Never` |
| `try_unwrap!` | recoverably unwrap an existing `Result` | `Option(R)` |
| `unwrap!` | necessarily unwrap an existing `Result` | `R` or `Never` |
| `fail!` | the current dynamic contract cannot produce its promised result | `Never` |
| `panic!` | an authored program invariant is broken | `Never` |

Recoverable domain alternatives remain ordinary data. A caller that needs to
continue computing a rejected branch uses `Option`, `Result`, or an application
enum whose error payload is also application data. The language failure
envelope is not that payload.

## Surface grammar

The parser recognizes only:

```text
dbg!(expression)
dbg!(expression, string-literal)
should_ok!(checker, arguments...)
must_ok!(checker, arguments...)
try_unwrap!(result)
unwrap!(result)
fail!(message, subjects...)
panic!(message)
```

`checker` must accept the supplied arguments and return `Result(R, String)`.
`fail!` requires a String message, which may use deterministic interpolation:

```telora
fail!(`ParseFailed at \{error.span}`, error, input)
```

The checker and each argument are evaluated once, from left to right. A
zero-argument checker is valid, but omitting the checker is not. `panic!`
accepts exactly one String expression and no subjects. `dbg!` retains RFC 0229.
`try_unwrap!` and `unwrap!` accept exactly one `Result(R, String)` expression.

Postfix contextual sugar from RFC 0230 remains uniform:

```text
receiver.ident!(args...) == ident!(receiver, args...)
```

For example, `check_order.should_ok!(a, b)` is
`should_ok!(check_order, a, b)`, `parse.must_ok!(input)` is
`must_ok!(parse, input)`, and
`"Missing".fail!(request)` is `fail!("Missing", request)`. Postfix sugar does
not introduce method lookup.

The removed names are ordinary unknown contextual intrinsics; they are not
reserved compatibility spellings.

## Typing and evaluation

For `checker: Fn(A1, ..., An) -> Result(R, String)`, matching arguments, a
String message, subjects `S1 ... Sn`, and value `A`:

```text
dbg!(value)                 : A
dbg!(value, literal)        : A
should_ok!(checker, arguments...) : Option(R)
must_ok!(checker, arguments...)     : R
try_unwrap!(result)                 : Option(R)
unwrap!(result)                     : R
fail!(message, subjects...) : Never
panic!(message)             : Never
```

Both check forms invoke the checker once. For `should_ok!`, `'Ok(result)` becomes
`'Some(result)` and `'Err(message)` publishes one warning whose evidence is the
ordered checker arguments, then becomes `'None`. For `must_ok!`, `'Ok(result)`
becomes `result`, while `'Err(message)` establishes a failure with the same
ordered evidence and produces `Never`. The checker therefore owns domain
validation and transformation; the intrinsic name chooses the recovery policy.
Whether a Host renders or stores a warning cannot be observed by Telora.

For example:

```telora
let (a, b) = check_order.should_ok!(a, b)?;
let config = parse_config.must_ok!(source);
```

The checker can be tested as an ordinary pure function. Neither form catches
`fail!` from inside the checker; that `Never` continues to propagate.

The value-level pair applies the same policies to an already computed result.
`try_unwrap!` maps `Ok` to `Some` and `Err(message)` to one warning plus `None`;
`unwrap!` maps `Ok` to its payload and `Err(message)` to a failure. The existing
Result value is the diagnostic evidence and is evaluated once. These forms
differ from `?`, which propagates the original `Result` or `Option` branch
without publishing a diagnostic or changing container family.

`fail!` evaluates message and subjects once, constructs an internal failure
envelope, and terminates the current semantic evaluation path. It never
produces a placeholder value, never widens a result to `Any`, and cannot be
caught as an exception by Telora code.

In best-effort evaluation, the failed expression nevertheless has an internal
evaluation result whose static and dynamic result type is `Never`. Any later
operation that depends on that result propagates `Never` without invoking user
code or a native operation. This result may inhabit the evaluator's dependency
graph, but it is not a materialized MainWorld/WorkWorld value and cannot be
published inside an array, record, tuple, module export, or final result.

`panic!` terminates the current path as an invariant failure. It carries only
its authored message and call location. It is not a substitute for expected
input or domain rejection.

## Provenance and internal lowering

check diagnostics and `fail!` share provenance rules:

- the complete authored invocation is the rule origin;
- each subject retains its data origin;
- multiple subjects remain ordered evidence;
- a check message comes from the checker's `Err(String)` result;
- a failure message is the first authored `fail!` argument;
- the internal envelope is never converted into a Telora value.

Conceptual lowering is:

```text
should_ok!(checker, args...) -> match checker(args...) {
                                  Ok(result) => Some(result),
                                  Err(message) => internal.warn(message, args...); None,
                               }
must_ok!(checker, args...)     -> match checker(args...) {
                                  Ok(result) => result,
                                  Err(message) => internal.raise(message, args...),
                               }
try_unwrap!(result)         -> match result {
                                Ok(value) => Some(value),
                                Err(message) => internal.warn(message, result); None,
                              }
unwrap!(result)             -> match result {
                                Ok(value) => value,
                                Err(message) => internal.raise(message, result),
                              }
fail!(message, subjects...) -> internal.raise(internal.blame(...))
panic!(message)             -> internal.panic(message, authored-location)
```

The old generic report operation and severity dispatch are removed rather than
retained under internal names. Warning and failure use distinct closed
operations.

## Best-effort evaluation

`fail!` makes dependent evaluation unnecessary, but does not require the Host
to stop the whole analysis request. A best-effort evaluator may continue other
authoritatively reachable evaluation units when they:

1. do not depend on the failed value;
2. remain reachable under the program's actual control-flow decision;
3. have all required authoritative inputs; and
4. do not duplicate speculative observations or diagnostics.

Dependent units are blocked; they are not executed with `Any`, defaults, or
synthetic values. Unselected branches are not evaluated for additional
diagnostics. The initial implementation may conservatively continue only
existing independently scheduled units; this RFC does not require general
function-body slicing.

Element-wise higher-order collection operations are a required best-effort
boundary. For example, diagnostic evaluation of two consecutive `array.map`
operations may internally progress as follows:

```text
[1, 2, 3]
[f1(1), Failed(f1(2)), f1(3)]
[f2(f1(1)), Failed(f1(2)), Failed(f2(f1(3)))]
```

`Failed` denotes the internal `Never` evaluation result; it is not a materialized
Telora value or an `Array` element. A later map propagates that slot as `Never`
without invoking its callback, while evaluating the remaining reachable
elements exactly once in stable index order. If any slot is `Never`, neither
intermediate nor final partial array may be published as `Array(A)`; the
complete expression is `Never`. Strict execution may stop at the first failure.
Diagnostic execution collects failures from independent element paths and then
propagates `Never` for the whole collection result. This requires callback
failure isolation in collection operations, but not speculative branch
execution or arbitrary slicing of a function body.

`panic!` marks the current module result untrustworthy. A Host may still inspect
already established independent module facts, but must not publish a successful
result for the panicked module.

Strict execution fails on any unhandled `fail!` or `panic!`. The exact recovery
and exit protocol of `telora check` remains owned by the dedicated check/test
protocol design; this RFC does not redefine it implicitly.

## Public library contracts

`BlameError`, `Severity`, and `report` leave the MainWorld/WorkWorld prelude and
module export surface. Public native and Telora modules must not expose the
internal failure envelope through `Result`.

`BlameError` itself remains a native opaque carrier owned by the evaluator and
Host. A `.native.telora` declaration may name it in a native binding contract
as Host ABI vocabulary. Ordinary `.telora` source does not receive the name in
its prelude and cannot name, construct, import, export, or store it directly.
Ordinary code may still match an inferred native error value and read the ABI
fields required to establish a domain or failure boundary, such as
`error.message`.

A native module must not re-export `BlameError` itself. It may expose a native
operation whose checked boundary contains `BlameError`, for example
`Result(A, BlameError)`, when ordinary callers need to decide whether to turn
that error evidence into `fail!(message, error, value)`. The carrier is produced
only by native/Host code; ordinary Telora cannot fabricate one. Semantic facts
may describe the declared native operation contract but must not publish a
standalone importable `BlameError` binding.

A future orchestration-oriented EdgeWorld may receive a capability to observe
or route this native carrier. That world, its authority, and its operations are
deferred; RFC 0231 does not expose `BlameError` in anticipation of it.

Each current `Result(A, BlameError)` native API is classified during migration:

- if callers legitimately inspect and continue from the error, define a
  module-owned ordinary error enum and return `Result(A, Error)`;
- if the operation cannot satisfy its declared result contract and callers do
  not have a meaningful recovery branch, fail internally;
- `.native.telora` and Host entry adapters may use the native opaque
  `BlameError` in native ABI contracts but cannot re-export the type binding;
  ordinary Telora decides a terminal boundary with `fail!(message, error, ...)`.

Application errors may contain spans, expected/actual values, causes, or repair
data. At the point a caller decides it cannot continue, it uses that ordinary
value as a subject:

```telora
match parse(source) {
    'Ok(value) => value,
    'Err(error) => fail!(`ParseFailed at \{error.span}`, error, source),
}
```

There is no `fail!(internal_error)` propagation overload. Every authored
failure boundary supplies a String message and zero or more ordinary evidence
values, establishing a fresh rule origin while preserving subject origins.

## Diagnostics and Host authority

Telora decides only value, checked recovery, failure, panic, and debug observation. The
Host owns diagnostic ordering, deduplication, rendering, JSONL shape, terminal
format, and command exit protocol. Host handling cannot turn a failed path into
a Telora value or turn a warning into a hidden control-flow edge.

There is no generic report operation. Recoverable domain checks return
`Result(R, String)` and cross `should_ok!` when warning plus filtering is
desired, or `must_ok!` when rejection makes the current result impossible. Direct
authored failures use `fail!`. Informational
development observation uses `dbg!`.

## Migration

This RFC is intentionally incompatible:

- recoverable `emit_warn!` sites become ordinary checkers returning
  `Result(R, String)`, used through `checker.should_ok!(arguments...)`;
- `emit_error!` sites that invalidate results become `fail!` and their
  surrounding return contracts are simplified where appropriate;
- `emit_info!` becomes `dbg!` only when it observes a value during development,
  otherwise it is removed;
- `raise!(blame!(m, xs...))` becomes `fail!(m, xs...)`;
- `'Err(blame!(...))` becomes a module/domain error enum or direct `fail!`;
- `report`, `Severity`, and `BlameError` are removed from MainWorld/WorkWorld
  contracts; the native opaque `BlameError` carrier remains internal.

Historical experiment snapshots and implemented RFC text remain historical
evidence. Current examples, standard modules, SSOT, tutorials, parser fixtures,
and executable tests are migrated.

## Rejected alternatives

### Retain `blame!` as an ordinary constructor

This keeps the internal Host envelope in the value language and permits errors
that are constructed but neither reported nor raised. Ordinary domain errors
already provide the required composability.

### Retain `raise!` for `BlameError`

This requires users to know and obtain the hidden envelope. An authored caller
instead establishes its own failure boundary with a message and ordinary error
evidence through `fail!`.

### Retain Error reporting without failure

It creates conflicting authorities: ordinary control flow publishes success
while the diagnostic account says Error. `should_ok!` covers typed recoverable
rejection; `must_ok!` and `fail!` cover an invalid current result.

`should!` and `must!` are reserved as a possible future predicate-adapter pair:
they would retain the checked input rather than unwrap an `Ok` payload. This RFC
does not accept or type those names because the retained shape for arbitrary
multi-argument predicates is not yet specified.

### Make every expected rejection a failure

Callers sometimes need to inspect, aggregate, retry, or transform rejection.
Those branches remain ordinary typed data until a caller decides its own result
contract cannot be fulfilled.

## Implementation plan

1. Close the contextual-intrinsic parser set and add `should_ok!`/`must_ok!`
   lowering.
2. Remove public blame/raise/emit forms and their parser/type/compiler tests.
3. Remove `Severity`, `BlameError`, and `report` from the ordinary source
   prelude, and remove generic report/severity dispatch; expose `BlameError`
   only while checking `.native.telora` declarations and reject its re-export.
4. Migrate public standard/native module contracts away from `BlameError`.
5. Migrate current examples and entry adapters; preserve historical snapshots.
6. Update LANGUAGE SSOT, language tutorial, CLI-facing examples, and grammar
   fixtures.
7. Run parser, compiler, module, CLI, LSP, and full workspace gates.

## Acceptance criteria

1. Exactly `dbg!`, `should_ok!`, `must_ok!`, `try_unwrap!`, `unwrap!`, `fail!`,
   and `panic!` are accepted contextual intrinsic names.
2. Removed names fail as unknown intrinsics in prefix and postfix forms.
3. Both check forms accept any arity
   `Fn(A1, ..., An) -> Result(R, String)` and evaluate the checker and arguments
   once in source order. `should_ok!` maps `Ok` to `Some` and `Err` to one Warn
   event plus `None`; `must_ok!` maps `Ok` to `R` and `Err` to one failure plus
   `Never`.
4. `try_unwrap!` and `unwrap!` apply the corresponding policies to one existing
   `Result(R, String)` value and evaluate it once.
5. `fail!` has `Never`, raises one structured internal failure, preserves rule
   and ordered subject provenance, and publishes no value.
6. `panic!` remains a distinct invariant failure with a String message.
7. No ordinary module exports or completion results expose `BlameError`,
   `Severity`, or `report`; `.native.telora` may name opaque `BlameError` only in
   native ABI contracts and cannot re-export its type binding.
8. Recoverable library failures use explicit module-owned error types; direct
   internal failures cannot be caught as Telora values.
9. Best-effort evaluation never runs a dependent unit with synthetic data and
   never evaluates an unselected branch to manufacture diagnostics.
10. Current standard modules, entry adapters, examples, LANGUAGE SSOT, and
   ontology injection tutorial use the new surface.
11. Historical experiment snapshots and earlier RFCs are not rewritten.
12. Full workspace tests, warning-denied Clippy, formatting, and diff checks
    pass.
