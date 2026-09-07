# Typed accumulation channels

- Stage: Discussion
- Possible successor: a future numbered RFC

## Question

Should a Telora call be able to produce a normal result while also emitting
typed auxiliary values that the caller may explicitly capture?

The motivating shape is:

```telora
let result = check(input);

let (result, diagnoses) = check(input; Diagnose);
```

The first form is an ordinary call. The second asks the call boundary to
capture `Diagnose` values emitted during that call and its transitive calls.
The function being called does not choose whether its accumulated values are
captured.

This document explores that model. It is not yet a syntax or implementation
commitment.

## Motivation

Many deterministic computations have a primary result and auxiliary output
that should not control the computation producing it:

- validation diagnostics and non-fatal warnings;
- provenance and explanation records;
- audit events;
- static-analysis observations;
- structured trace information intended for tools rather than program flow.

Returning all of this explicitly makes every intermediate function unpack,
merge, and repack arrays it does not otherwise use:

```telora
{
    value: checked,
    diagnoses: diagnoses,
}
```

A mutable global collection avoids that plumbing but violates Telora's closed,
deterministic computation model. Typed accumulation channels attempt to retain
the useful part of an auxiliary output effect without introducing readable
ambient state.

## Provisional model

An accumulation channel has two phases:

```text
while a call is executing       after the call completes
write-only channel          ->  sealed read-only values
```

Code executing inside the call may append a value but cannot inspect the
current contents of the channel. A successful capturing call seals the
captured values and returns them to its caller alongside the primary result.

Conceptually:

```text
F: Fn(A) -> R

F(a)                  : R
F(a; Diagnose)        : (R, Array(Diagnose))
F(a; Diagnose, Trace) : (R, Array(Diagnose), Array(Trace))
```

The semicolon separates ordinary function arguments from accumulation channels
selected by the caller. Channel selectors are not arguments visible to `F`.

An illustrative declaration and emission syntax is:

```telora
@accumulator type Diagnose = {
    message: String,
    path: String,
};

def check_name = fn(name) {
    if string.is_empty(name) {
        accumulate Diagnose({
            message: "name must not be empty",
            path: "name",
        });
    };

    name
};
```

Both `@accumulator type` and `accumulate Channel(value)` are provisional. The
semantic distinction between a channel identity and its payload type remains
an open question.

## Call-boundary behavior

The current candidate has the following behavior.

### Ordinary calls propagate

An ordinary call does not discard accumulated values:

```telora
let value = check(input);
```

Values emitted by `check` and its transitive calls continue into the nearest
enclosing capture boundary, or into the root execution result if no Telora call
captures them.

### Capturing calls intercept selected channels

```telora
let (value, diagnoses) = check(input; Diagnose);
```

This captures `Diagnose` values emitted during the dynamic extent of
`check(input)`. Values from channels not listed after the semicolon continue to
propagate outward.

### The nearest boundary wins

If an inner call captures a channel, those sealed values do not also propagate
to an outer capture of the same channel:

```telora
def outer = fn(input) {
    let (value, inner_diagnoses) = inner(input; Diagnose);
    accumulate Diagnose({ message: "outer", path: "" });
    value
};

let (value, outer_diagnoses) = outer(input; Diagnose);
```

`inner_diagnoses` contains values from `inner`. `outer_diagnoses` contains the
value emitted directly by `outer`, but not values already captured by the
inner call.

### Argument evaluation is outside the boundary

In:

```telora
foo(make_input(); Diagnose)
```

`make_input()` is evaluated before the `Diagnose` capture boundary is entered.
Its accumulated values therefore belong to the caller's surrounding context.
The boundary covers the execution of `foo`, including callbacks invoked by
`foo`, but not evaluation performed to prepare `foo`'s arguments.

### Completion seals the result

Only successful completion produces a sealed accumulated result. Cancellation,
fuel exhaustion, quota failure, or another failed execution must not expose a
partially accumulated result as if it were complete. Whether failure APIs may
optionally expose explicitly marked partial observations is outside the initial
model.

## Relationship to ordinary results

Accumulation is not a replacement for `Result`, `Option`, tuples, or ordinary
collections.

Information belongs in the primary result when the caller must inspect it to
continue correctly. Accumulation is appropriate when the producing computation
must not read the channel and the information is observational or auxiliary.

For example, decoding returns `Result(A, codec.BlameError)` so callers can
recover or emit a diagnostic with `raise!(error)`.
Non-fatal migration warnings or provenance records may be accumulated.

This distinction prevents an API from hiding its essential failure contract in
a channel that an ordinary caller can ignore.

## Relationship to algebraic effects

The model can be understood as a deliberately restricted typed effect:

```text
accumulate<Channel>(value) -> Unit
```

The call suffix installs a handler for selected channels, but the handler is
restricted:

- it cannot inspect values until the call completes;
- it cannot capture, suppress, duplicate, or resume a continuation;
- the operation always continues exactly once and returns `Unit`;
- it cannot replace the primary return value;
- it only collects values in deterministic evaluation order.

This is substantially smaller than general algebraic effects. Introducing
local handlers, effect polymorphism, continuation reification, or custom
reducers is not implied by this proposal.

## Relationship to Salsa accumulators

Salsa demonstrates that auxiliary values can be stored separately from a
query's primary memo and collected transitively through a dependency graph.
That is useful prior art, especially for diagnostics, replacement on
re-execution, and reuse of unchanged work.

The proposed Telora model deliberately gives the capture decision to each call
site. It does not require a function to be declared as a query, does not define
collection by a memoized query key, and does not make global memoization part
of ordinary function semantics. Its initial collection boundary is the dynamic
extent of the selected call.

The same surface semantics could later be optimized by an incremental engine,
but such an optimization must preserve call-boundary behavior.

## Type-system questions

### Channel identity

A channel must have stable identity across modules. Structural equality alone
is insufficient because two channels may intentionally use equal payload
shapes while remaining independently capturable.

Two candidate models are:

```telora
@accumulator type Diagnose = { message: String };
```

or separate payload and channel declarations:

```telora
@struct type Diagnostic = { message: String };
accumulator Diagnose: Diagnostic;
```

The first is concise. The second permits multiple channels carrying the same
payload type and keeps data type identity separate from effect identity.

### Function effect metadata

Potentially accumulated channels may eventually appear in function metadata:

```text
Fn(Input) -> Output accumulates(Diagnose, Trace)
```

This would improve API documentation, hover, auditing, separate compilation,
and optimization correctness. It also introduces transitive effect inference,
effects from callbacks, and compatibility rules for higher-order functions.

An initial design could permit callers to capture any declared channel and
produce an empty Array when no value is emitted, without requiring complete
effect inference. The RFC must decide whether that is a temporary staging rule
or the permanent contract.

### Result shape

The positional candidate is concise:

```telora
let (value, diagnoses, traces) = work(input; Diagnose, Trace);
```

Its result type follows the requested order. Alternatives include returning a
record keyed by channel or introducing a dedicated execution-result type.
Those alternatives may be easier to evolve but are more verbose and may
require generated structural types.

### Higher-order calls

A callback invoked within the selected call is inside its dynamic extent:

```telora
map(values, fn(value) {
    accumulate Diagnose(...);
    transform(value)
}; Diagnose)
```

The design must specify how callback effects appear in function metadata if
effect checking is introduced. The runtime behavior itself can remain simple:
the callback writes to the nearest active boundary for that channel.

## Determinism and resource behavior

Within sequential Telora evaluation, captured values should retain emission
order. Duplicate values are meaningful and are not removed automatically.

Future parallel evaluation must not order values by thread completion time.
It would need a deterministic merge rule derived from semantic evaluation
order, or it must decline to parallelize computations whose ordered accumulated
output is observable.

Accumulated values consume VM resources and must count toward allocation and
collection quotas. A program must not obtain unbounded storage merely because
the values are auxiliary. Exact per-channel and total limit policies belong in
the RFC and implementation plan.

## Alternatives

### Return arrays explicitly

This requires no language feature and keeps every output visible in the
ordinary return type. It is preferable for essential business results, but it
forces unrelated intermediate layers to transport auxiliary output.

### Pass an explicit collector

```telora
check(input, collector)
```

This makes capability flow explicit but exposes mutable or linear state in
ordinary values and requires every intermediate API to accept and forward the
collector.

### Root-only accumulation

Only the embedding host could read accumulated values after the entire Telora
execution. This is simpler, but Telora libraries could not establish abstraction
boundaries or transform diagnostics produced by a lower-level call.

### Query-key accumulation

A Salsa-like `query::accumulated(Channel, args)` could retrieve values after a
memoized query. This is attractive for an incremental query system but requires
query identity and memo semantics to become part of the language model. It
also takes the capture decision away from the immediate call site.

### General effect handlers

General handlers would subsume accumulation but introduce continuation and
effect-polymorphism complexity that the motivating scenarios do not require.

## Open questions

1. Is a channel also its payload type, or are channel and payload declared
   separately?
2. Is `(R, Array(A), Array(B))` the right result shape for `f(x; A, B)`?
3. Must function metadata declare or infer every channel it can accumulate?
4. If effect metadata is present, how are callback effects represented?
5. Is capturing a channel that cannot currently be emitted always valid and
   guaranteed to produce an empty Array?
6. Does the root execution always retain all uncaptured channels, and how does
   the embedding API address them?
7. Are accumulated values strictly ordered, or should some channels be
   explicitly unordered to permit parallel evaluation?
8. What quota applies to a channel, and what failure is reported when it is
   exceeded?
9. Should duplicate channel selectors be a static error?
10. Which syntax best distinguishes declaration, emission, propagation, and
    capture without making accumulation resemble an ordinary argument?

## Criteria for promotion to an RFC

This discussion is ready to become an RFC when it can answer at least:

1. the source syntax for declaring, emitting, and capturing a channel;
2. the nominal identity and cross-module representation of a channel;
3. the exact return type of single- and multi-channel capturing calls;
4. nested propagation and interception semantics;
5. argument, callback, branch, and failure-boundary behavior;
6. whether and how accumulation appears in `Fn` metadata;
7. deterministic ordering and quota guarantees;
8. the root embedding protocol for uncaptured channels;
9. static-analysis, LSP, compiler, and VM acceptance cases;
10. explicit non-goals that keep the feature smaller than general algebraic
    effects.
