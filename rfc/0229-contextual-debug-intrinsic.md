# RFC 0229: Contextual `dbg!` observation

- Status: Accepted for implementation
- Replaces: RFC 0019
- Tracking issue: #52

## Summary

Telora replaces the context-free `std/debug` functions with one contextual
intrinsic:

```telora
dbg!(expression)
dbg!(expression, message)
```

The intrinsic evaluates `expression` exactly once, offers one bounded debug
event to the Host, and returns the exact resulting value with its exact static
type. The optional `message` is an authored String literal captured by the
compiler; it is not a second runtime expression.

Every event carries compiler-authored context for the first argument:

- the stable logical module identity;
- the complete `dbg!` call location;
- the source text of `expression` as authored;
- the optional authored message; and
- the existing bounded, deterministic representation of the resulting value.

CLI execution renders events to stderr. Final program output remains on stdout.
Embedding Hosts may capture or discard events through the existing debug sink.

The public `std/debug` module, `debug.dbg`, and `debug.dbg_with` are removed
without compatibility aliases.

## Motivation

The RFC 0019 observer can display an arbitrary runtime value without changing
it, but it receives only an already evaluated value:

```telora
debug.dbg(value)
debug.dbg(request.user)
debug.dbg(make_plan(model, request))
```

Without a manually repeated label, these calls produce indistinguishable
records apart from their values. A hand-written label can become stale after a
rename and still does not identify the authored module or call site. This made
the existing API useful as a formatter test but incomplete as a source-level
debugging tool.

Source context is known statically at the intrinsic call. It should be captured
there, as `blame!` captures its authored rule site, rather than reconstructed
through runtime stack reflection or copied manually into ordinary Strings.

## Surface syntax

`dbg!` is a reserved contextual intrinsic with exactly one of these arities:

```text
dbg!(expression)
dbg!(expression, message)
```

Zero arguments and more than two arguments are syntax diagnostics. The first
argument may be any expression. The second argument must be a String literal.
Interpolation, concatenation, calls, variables, and other runtime String
expressions are rejected in this slot. Dynamic context belongs in the observed
first argument.

`dbg!` is an expression and may occur anywhere its first argument may occur:

```telora
let plan = dbg!(make_plan(model, request));
let checked = dbg!(plan, "before validation");

request
|> make_plan(model, _)
|> dbg!(_, "generated plan")
|> lower_sql
```

There is no imported `dbg` binding, no implicit prelude function, and no
`std/debug` module. The bang spelling states that authored context participates
in the operation and distinguishes it from an ordinary polymorphic identity
function.

## Evaluation and typing

For an expression with type `A` and an optional String literal `message`:

```text
dbg!(expression)          : A
dbg!(expression, message) : A
```

The checker propagates the surrounding expected type into `expression` and
retains the resulting exact type. It does not route the value through `Any`,
`Dyn`, export, codec, or runtime reflection.

Runtime semantics are:

1. evaluate `expression` exactly once;
2. offer its value and compiler-authored context to the Host observer;
3. return the exact runtime value produced by step 1.

If `expression` fails, no event is offered. Debug observation is ordered with
other debug observations and cannot be removed, reordered, duplicated, or
common-subexpression-eliminated across authored calls.

The observer is absent from the Telora semantic world. Whether a Host installs
a sink, discards an event, truncates its rendering, or fails to write it cannot
be observed by Telora code and cannot change success, failure, value identity,
types, diagnostics, allocation accounting, or control flow. The formatter and
sink have no Telora-visible return channel.

The intrinsic observes only the explicitly supplied expression. It does not
capture local scope, closure environments, parameters, imported bindings, or
other nearby values.

## Authored context

The parser handles `dbg!` through the same closed contextual-intrinsic surface
as `blame!`. It retains two related source ranges:

- the complete invocation, used as the event call location; and
- the first argument, used to recover its authored expression text.

The compiler resolves the source to the stable logical module identity already
used by diagnostics and semantic snapshots. The runtime receives compiler-
authored metadata; it does not inspect a caller stack or open source files.

Expression text is the exact UTF-8 source slice covered by the first argument,
including authored whitespace inside that range. It is context, not a Telora
String value and not part of value equality. Synthetic expressions without an
authored slice use the stable placeholder `<generated>`.

Module identity and source positions follow the same logical-source mapping as
other diagnostics. Line and column are one-based. Physical absolute paths are
not exposed when a stable module identity exists.

## Event and CLI rendering

The Host-facing event is conceptually:

```text
DebugEvent {
    module: String,
    location: Location,
    expression: String,
    message: Option<String>,
    value: String,
}
```

`expression`, `message`, and `value` are owned, bounded text. The value uses the
existing cycle-safe debug formatter, including deterministic Dict ordering and
fixed depth, item-count, and byte limits. The formatter remains a debug
representation, not JSON or a stable serialization protocol.

The CLI writes one physical stderr record per event. Its logical form is:

```text
[debug] @src/query.telora:42:12 make_plan(model, request) = {...}
[debug] @src/query.telora:43:12 "before validation": plan = {...}
```

Control characters in messages, expression text, and value text are escaped so
one event cannot forge additional records. Exact cosmetic punctuation is a CLI
presentation detail; the module, position, expression, optional message, and
value are required information.

Debug events never enter stdout, the module export record, diagnostics, JSON
codec output, or the `output` entry protocol. A Host sink cannot change Telora
control flow or return a value to the program.

## Tool and program evaluation

`dbg!` may execute wherever an authored Telora expression is authoritatively
evaluated, including metadata/tool evaluation and program execution. An
internal speculative, bootstrap, recovery, or repeated analysis pass must not
publish an additional event as though it were an authored evaluation.

This RFC does not add an evaluation-stage field to the public event. The stable
module and call location identify the authored observation; a later tracing RFC
may add explicit stage or span metadata if a demonstrated use case requires it.

## Fuel and limits

Evaluating the first argument consumes its ordinary language work. The
observation itself consumes no Telora fuel, stack, or allocation quota because
charging it could make observer presence visible through program success or
failure. Formatting and event ownership are bounded Host work and allocate no
Telora value. Hosts may apply their own observer-output limits, but reaching
such a limit only drops or truncates observation and cannot fail evaluation.

Large and sensitive values remain the author's responsibility. Fixed formatter
limits bound accidental output volume but are not a secrecy boundary. The
language does not redact values or enable debug output conditionally in this
RFC.

## Lowering and implementation

`blame!` establishes the frontend pattern: a reserved contextual form is parsed
without name resolution and receives compiler-authored provenance for its full
call site. `dbg!` reuses that closed parser path and source-location discipline.

Unlike `blame!`, `dbg!` cannot lower completely to an ordinary Dict because it
must emit a Host event while returning the original value. The AST/HIR therefore
retains a dedicated debug-observation expression containing:

```text
Debug {
    value,
    static_message?,
    call_location,
    expression_location,
}
```

LIR and bytecode retain a dedicated observation operation so optimization and
runtime execution preserve exactly-once ordering. Static context is stored in
the compiled function or referenced through its source map. The VM sends that
context and the formatted value through the existing injected `DebugSink`.

The current `CoreDebugFunction`, `std/debug.native.telora`, and native function
dispatch are removed. The existing formatter, sink injection, event capture,
and stderr boundary are retained and adapted to the contextual event.

## Documentation

The language SSOT defines `dbg!` as the language's temporary value-observation
form. The injected language tutorial includes runnable examples and states:

- debug output goes to stderr and the original value continues through the
  expression or pipeline;
- debug representation is bounded and is not a serialization contract;
- codec is used for stable structured boundaries;
- domain summaries are explicit long-lived human interfaces; and
- `dbg!` should not expose sensitive values or remain as accidental production
  logging.

The CLI guide documents stderr behavior without treating `dbg!` as part of the
`run` output protocol.

## Rejected alternatives

### Keep both the module functions and intrinsic

Two overlapping observation surfaces would preserve the context-free path and
force users to choose between pipeline ergonomics and useful source context.
The intrinsic already works in pipelines through `_`, so the module adds no
distinct language capability.

### Infer a variable name at runtime

Runtime values do not retain a unique lexical binding. One value may come from
a field projection, call, branch, alias, or generated expression. Runtime stack
reflection would be ambiguous and would expose a much broader capability than
the authored intrinsic needs.

### Capture every local variable

Implicit scope capture reads values the author did not select, risks leaking
sensitive data, changes what remains observable after optimization, and creates
unbounded output. `dbg!` observes exactly one explicit expression.

### Expand to `debug.dbg_with(source_text, value)`

Ordinary Strings cannot carry the authoritative module and source provenance
without exposing forgeable compiler context as program data. An ordinary call
also obscures the observation effect from optimization and tooling.

### Make debug output JSON

The runtime value domain is broader than JSON and debugging must remain usable
for functions, recursive graphs, Bytes, and incomplete internal values. Stable
JSON boundaries belong to codec and schema contracts.

## Implementation plan

1. Add `dbg!` to the closed contextual-intrinsic parser with exact arity,
   String-literal message validation, and source-range preservation.
2. Add exact type checking and expected-type propagation for the observed
   expression.
3. Add dedicated AST/HIR, LIR, bytecode, and VM observation forms carrying or
   resolving compiler-authored context.
4. Adapt `DebugEvent`, the bounded formatter, Engine sink injection, and CLI
   stderr rendering to contextual records.
5. Remove `std/debug`, its native identities, dispatch, module registration,
   and context-free tests.
6. Add parser, type, compiler, VM, module, CLI, recovery, and pipeline tests.
7. Update the language SSOT, injected language tutorial, and CLI guide.

## Acceptance criteria

1. `dbg!(value)` and `dbg!(value, "message")` parse and check without an import;
   invalid arity and every non-literal message form are rejected.
2. The observed expression is evaluated exactly once; its failure offers no
   event to the Host.
3. The result retains the exact static type and exact runtime identity of the
   observed value, including generic, function, recursive, and composite values.
4. Every event identifies the stable logical module, one-based call location,
   authored first-argument text, optional message, and bounded value text.
5. Pipeline use through `dbg!(_, "message")` works without weakening inference.
6. CLI debug records go only to stderr; stdout and exported `output` are
   unchanged.
7. Cycles and formatter limits remain deterministic and cannot panic.
8. Tool-stage authoritative evaluation may emit an event; speculative,
   bootstrap, and recovery work does not publish duplicate observations.
9. Importing `std/debug` fails because the module and both old functions no
   longer exist.
10. The SSOT, tutorial, and CLI guide describe only the new intrinsic.
11. Installing, discarding, truncating, or failing a Host debug sink cannot
    alter any Telora-visible result, failure, diagnostic, quota, or type fact.
