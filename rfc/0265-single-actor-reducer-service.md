# RFC 0265: Single-Actor Reducer Service

- Status: Accepted
- Tracking: #147
- Depends on: RFC 0257, RFC 0264

## Summary

Every application execution contains one Telora actor. The actor owns one explicit state value and
one reducer:

```text
reduce(State, Event) -> Tuple([State, Array(Effect)])
```

Application execution may also construct one native EES actor. Its manifest contains multiple
named native actor models such as IMOS and `sqlite-query`. EES models do not own Telora sessions,
mailboxes or independent Telora lifecycles. The Host and this EES actor run in one process and
interact through the Rust facade; this RFC defines no inter-process transport between them.

`run` and `serve` use the same actor protocol. `run` submits one input request and exits after its
reply. `serve` submits each transport input as another request and remains active until its input
stream ends.

Tools select ordinary module exports by nominal type. `eval` reads one exported `Value`;
`eval-with`, `run` and `serve` accept values constructed by `std/entry`. Module resolution only
establishes identity and visibility. A resolvable export is usable by a tool only when its type
matches that tool's contract.

## Public service protocol

`std/actor` defines first-order request, event and effect values:

```telora
type Request = struct { id: String, input: Value };
type EesReply = struct {
    id: String,
    request_id: String,
    result: Result(Value, String),
};
type Event = enum {
    'Request(Request),
    'EesReply(EesReply),
};
type EesCall = struct {
    id: String,
    request_id: String,
    request: ees.Request,
};
type Reply = struct { request_id: String, value: Value };
type Effect = enum {
    'EesCall(EesCall),
    'Reply(Reply),
};
type Transition(State) = Tuple([State, Array(Effect)]);
```

An EES request remains component-neutral data:

```telora
type ees.Request = Tuple([String, String, Value]);
```

Its fields select a named model, an operation and operation input. The application supplies a call
ID and the originating request ID when it creates an `EesCall` effect.

## Tool-stage wrappers

`std/entry` defines ordinary typed configuration values and nominal tool wrappers:

```telora
type ContextConfig = struct {
    sources: Array(String),
    envs: Array(String),
    args: Bool,
};
type Context = struct {
    sources: Dict(Value),
    env: Dict(String),
    args: Array(String),
};
type Eval;
type Run(State);
type Serve(State);
```

Their constructors have these conceptual contracts:

```text
entry.main:
  Fn(ContextConfig, Fn(Context) -> Value) -> Eval

entry.run:
  for(State) Fn(
    ContextConfig,
    Ees,
    Fn(Context) -> Tuple([
      State,
      Fn(State, actor.Event) -> actor.Transition(State),
    ]),
  ) -> Run(State)

entry.serve:
  for(State) Fn(
    ContextConfig,
    Ees,
    Fn(Context) -> Tuple([
      State,
      Fn(State, actor.Event) -> actor.Transition(State),
    ]),
  ) -> Serve(State)
```

`State` remains transparent in the exported `Run(State)` or `Serve(State)` type. The factory and
reducer use the concrete application type and need no explicit `TypeOf(State)` argument. When a
Host selects the export, it verifies the nominal wrapper family, reads its concrete type argument
and performs one tool-stage existential erasure. This boundary produces the same runtime shape as
an `actor.Service`: a `Dyn` state and a reducer wrapper over `(Dyn, Event)`.

The wrapper family is constructor-controlled by `std/entry`. A structurally similar user record is
not an entry capability. This staging rule does not add implicit type witnesses to ordinary Telora
calls.

The erased shape is:

```telora
type Service = struct {
    state: Dyn,
    reduce: Fn(Tuple([Dyn, Event])) -> Tuple([Dyn, Array(Effect)]),
};
```

Type erasure is limited to the Host tool boundary. Application reducer code retains its concrete
State type.

## Host lifecycle

The Host selects `MODULE:EXPORT`, evaluates the closed module, verifies `EXPORT: Run(State)` or
`EXPORT: Serve(State)`, and reads its `ContextConfig` and `Ees` declarations before acquiring
physical resources. It then builds `Context`, calls the wrapper factory once and erases the
resulting concrete state at the tool boundary.

The run Entry translates initialization into exactly one actor request:

```text
Request { id: "run", input: None }
```

It drives EES call/reply transitions until the actor emits the matching `Reply`, writes that Value
as JSON and exits. A second application reply or an effect after the terminal reply is invalid.

The serve Entry assigns a stable process-local ID to each JSONL input, sends a `Request`, and maps
the matching `Reply` to one JSONL response. EES replies may arrive out of order. The Entry tracks
only protocol bookkeeping: request IDs, EES call IDs and accumulated diagnostics. Business state
belongs to the application State.

Both Entries translate actor `EesCall` into the private `SystemEffect.EesCall` and translate the
correlated private `SystemEvent.EesReply` back into an actor event. The native EES actor receives
only model, operation and JSON-compatible input data.

## Pure evaluation lifecycle

`eval MODULE:NAME` requires `NAME: Value`. `eval-with MODULE:NAME` requires `NAME: entry.Eval`.
The `entry.Eval` value contains a `ContextConfig` and a function from `Context` to `Value`. Source
and environment names must match the typed configuration exactly; undeclared environment
variables remain invisible. Trailing CLI arguments are admitted only when `config.args` is true.

Eval sources use the same CST validation, data limits and semantic `Value` materialization as Entry
sources. Their canonical names use `@eval-ctx/<key>`, while physical locators remain Host-private.
The evaluator selects or invokes the export directly in a WorkWorld and serializes the resulting
`Value` without interpreting any effect.

## Module and tool selection

All four tools use one selector syntax:

```text
telora eval      MODULE:EXPORT
telora eval-with MODULE:EXPORT
telora run       MODULE:EXPORT
telora serve     MODULE:EXPORT
```

Ordinary resolver rules determine whether `MODULE` exists and is visible. The selected definition
must be exported. The tool then enforces its nominal value contract. Source directories do not
grant executable identity or additional resolver authority; `@bin`, `src/bin`, `src/entry` and
entry-specific source suffixes are not part of the language-level selection model. Packaging may
name or bundle a selected export, but that artifact concern does not change module semantics.

Crate identity and dependencies come from `telora-crate.json` and the exact workspace lock graph.
Runtime capabilities come from the selected `std/entry` wrapper. Telora source has no `option`
declaration and therefore cannot introduce a second configuration channel.

## Reducer failures and diagnostics

The standard Entry invokes the application reducer through the runtime diagnostic boundary.
Diagnostics are associated with the request that caused a transition. A successful serve reply
contains the accumulated diagnostics for that request. A failed request produces an error response
and removes its Entry protocol bookkeeping. Reducer state changes are committed only when the
transition succeeds.

Expected native component failures arrive as `EesReply.result = Err(String)` and are ordinary
application events. Applications decide whether to reply with data, issue another effect or raise a
diagnostic.

## Invariants

- One application execution owns exactly one Telora actor State and reducer.
- State remains transparent through `Run(State)` or `Serve(State)` and is erased only by the Host.
- Every successful transition explicitly returns the complete next State.
- Per-request continuation data is first-order State, not a captured callback.
- Effects and events contain no Telora function values.
- One application EES actor owns all named native models for the execution.
- An EES operation cannot create a model or choose a physical resource.
- `run` and `serve` differ only in request production and termination policy.
- Package preparation's private EES instance is outside the application runtime topology.
- Pure eval accepts and returns `Value` but does not create an application runtime topology.
- Resolution, export visibility and tool eligibility are separate checks.

## Verification

Executable tests establish:

- `run` creates one request and completes only after its matching Reply;
- explicit State sequences multiple EES calls and replies without callbacks;
- duplicate active call IDs, duplicate replies and replies with active calls fail;
- `serve` updates one explicit State, correlates concurrent replies and keeps diagnostics local to
  the originating request;
- application EES resources remain separate from package preparation's private IMOS service;
- `eval` accepts only a `Value` export;
- `eval-with` enforces its nominal wrapper type, source/env declarations, canonical source names
  and JSON/YAML/TOML data admission;
- `run` and `serve` accept ordinary module exports with transparent concrete State types;
- resolvable exports with the wrong nominal wrapper type are rejected by the selected tool;
- the public actor protocol is exported entirely as first-order data plus the service reducer.

## Deferred work

This RFC does not add multiple Telora actors, actor addresses, actor-to-actor effects, actor fork,
migration, supervision, mailboxes, Highway, shared ring buffers, distributed execution or an
independent process/session lifecycle for each EES model.

The hidden `telora ees` command and its stdio protocol are development fixtures for exercising EES
components independently. They are not a language, application-runtime or compatibility surface.

A future JSON packaging descriptor may bind an exact lock graph, a tool kind, one
`MODULE:EXPORT` entry and optional bundled modules into a directly runnable artifact. The Host
constructs the locked module world, resolves the named export, and still validates its nominal
tool type. The descriptor remains a packaging surface: it does not give source directories or
filenames execution identity and does not change resolver semantics.
