# RFC 0231: Closed diagnostic surface

- Status: Accepted
- Tracking issue: #59
- Replaces the public surfaces of RFCs 0105, 0188, and 0189

## Summary

Telora exposes exactly four contextual diagnostic intrinsics:

```telora
dbg!(value)
dbg!(value, "message")
warn!(message, subjects...)
fail!(message, subjects...)
panic!(message)
```

The list contains four intrinsic names; `dbg!` has two arities. No compatibility
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

The closed surface instead separates four meanings:

| Surface | Meaning | Result |
| --- | --- | --- |
| `dbg!` | observe an explicitly valid value during development | the same `A` |
| `warn!` | the result remains valid but deserves Host attention | `'None` |
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
warn!(message, subjects...)
fail!(message, subjects...)
panic!(message)
```

`message` in `warn!` and `fail!` is an ordinary expression checked as `String`.
It may use deterministic interpolation:

```telora
fail!(`ParseFailed at \{error.span}`, error, input)
```

Each subject is evaluated once, from left to right. Zero subjects are valid.
Zero total arguments are rejected because a message is mandatory. `panic!`
accepts exactly one String expression and no subjects. `dbg!` retains RFC 0229.

Postfix contextual sugar from RFC 0230 remains uniform:

```text
receiver.ident!(args...) == ident!(receiver, args...)
```

For example, `"Missing".fail!(request)` is
`fail!("Missing", request)`. Postfix sugar does not introduce method lookup.

The removed names are ordinary unknown contextual intrinsics; they are not
reserved compatibility spellings.

## Typing and evaluation

For `message: String`, subjects `S1 ... Sn`, and value `A`:

```text
dbg!(value)                 : A
dbg!(value, literal)        : A
warn!(message, subjects...) : enum {None}
fail!(message, subjects...) : Never
panic!(message)             : Never
```

`warn!` evaluates message and subjects once, constructs an internal warning
envelope, offers it to the Host diagnostic account, and returns `'None`.
Whether a Host renders or stores the warning cannot be observed by Telora.

`fail!` evaluates message and subjects once, constructs an internal failure
envelope, and terminates the current semantic evaluation path. It never
produces a placeholder value, never widens a result to `Any`, and cannot be
caught as an exception by Telora code.

`panic!` terminates the current path as an invariant failure. It carries only
its authored message and call location. It is not a substitute for expected
input or domain rejection.

## Provenance and internal lowering

`warn!` and `fail!` share one internal blame construction protocol:

- the complete authored invocation is the rule origin;
- each subject retains its data origin;
- multiple subjects remain ordered evidence;
- message is a deterministic String, not an error category registry;
- the internal envelope is never converted into a Telora value.

Conceptual lowering is:

```text
warn!(message, subjects...) -> internal.warn(internal.blame(...))
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
its prelude and cannot construct, inspect, import, export, or store it directly.

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

Telora decides only value, warning, failure, panic, and debug observation. The
Host owns diagnostic ordering, deduplication, rendering, JSONL shape, terminal
format, and command exit protocol. Host handling cannot turn a failed path into
a Telora value or turn a warning into a hidden control-flow edge.

There is no generic report operation. If the result is invalid, use `fail!`; if
it remains valid, use `warn!`. Informational development observation uses
`dbg!`.

## Migration

This RFC is intentionally incompatible:

- `emit_warn!(m, xs...)` becomes `warn!(m, xs...)`;
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
while the diagnostic account says Error. `warn!` covers valid results and
`fail!` covers invalid ones.

### Make every expected rejection a failure

Callers sometimes need to inspect, aggregate, retry, or transform rejection.
Those branches remain ordinary typed data until a caller decides its own result
contract cannot be fulfilled.

## Implementation plan

1. Close the contextual-intrinsic parser set and add `warn!` lowering.
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

1. Exactly `dbg!`, `warn!`, `fail!`, and `panic!` are accepted contextual
   intrinsic names.
2. Removed names fail as unknown intrinsics in prefix and postfix forms.
3. `warn!` returns `'None`, reports one Warn event, and evaluates each argument
   once in source order.
4. `fail!` has `Never`, raises one structured internal failure, preserves rule
   and ordered subject provenance, and publishes no value.
5. `panic!` remains a distinct invariant failure with a String message.
6. No ordinary module exports or completion results expose `BlameError`,
   `Severity`, or `report`; `.native.telora` may name opaque `BlameError` only in
   native ABI contracts and cannot re-export its type binding.
7. Recoverable library failures use explicit module-owned error types; direct
   internal failures cannot be caught as Telora values.
8. Best-effort evaluation never runs a dependent unit with synthetic data and
   never evaluates an unselected branch to manufacture diagnostics.
9. Current standard modules, entry adapters, examples, LANGUAGE SSOT, and
   ontology injection tutorial use the new surface.
10. Historical experiment snapshots and earlier RFCs are not rewritten.
11. Full workspace tests, warning-denied Clippy, formatting, and diff checks
    pass.
