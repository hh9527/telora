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
mailboxes or independent Telora lifecycles.

`run` and `serve` use the same actor protocol. `run` submits one input request and exits after its
reply. `serve` submits each transport input as another request and remains active until its input
stream ends.

Pure module evaluation has a separate CLI surface. `eval` reads one exported `Value`, while
`eval-with` invokes one function of type
`Fn({sources: Dict(Value), env: Dict(String), args: Array(String)}) -> Value`. Neither command
constructs an Entry, RunHost, application EES actor or effect loop.

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

## Typed state erasure

Applications construct the standard Entry boundary with:

```telora
actor.service(State, initial_state, reduce)
```

The first argument is `TypeOf(State)`. `std/actor` stores the current state as `Dyn` and wraps the
typed reducer once. The wrapper projects the current state, calls the same reducer and packs the
returned next state. The state remains separate from the reducer. Effect values contain no
function, callback or continuation.

The erased shape is:

```telora
type Service = struct {
    state: Dyn,
    reduce: Fn(Tuple([Dyn, Event])) -> Tuple([Dyn, Array(Effect)]),
};
```

Type erasure is limited to the standard Entry boundary. Application reducer code retains its
concrete State type.

## Host lifecycle

Both standard Entries initialize sources and call:

```telora
service: Fn(Dict(Value)) -> actor.Service
```

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

`eval MODULE:NAME` accepts only an `@src` module selector and requires `NAME: Value`.
`eval-with MODULE:NAME` requires a monomorphic context function returning `Value`. Its source and
environment capabilities are declared by `eval-ctx.sources` and `eval-ctx.env`; source names must
match exactly and undeclared environment variables remain invisible. Trailing CLI arguments enter
the context unchanged.

Eval sources use the same CST validation, data limits and semantic `Value` materialization as Entry
sources. Their canonical names use `@eval-ctx/<key>`, while physical locators remain Host-private.
The evaluator selects or invokes the export directly in a WorkWorld and serializes the resulting
`Value` without interpreting any effect.

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
- Every successful transition explicitly returns the complete next State.
- Per-request continuation data is first-order State, not a captured callback.
- Effects and events contain no Telora function values.
- One application EES actor owns all named native models for the execution.
- An EES operation cannot create a model or choose a physical resource.
- `run` and `serve` differ only in request production and termination policy.
- Package preparation's private EES instance is outside the application runtime topology.
- Pure eval accepts and returns `Value` but does not create an application runtime topology.

## Verification

Executable tests establish:

- `run` creates one request and completes only after its matching Reply;
- explicit State sequences multiple EES calls and replies without callbacks;
- duplicate active call IDs, duplicate replies and replies with active calls fail;
- `serve` updates one explicit State, correlates concurrent replies and keeps diagnostics local to
  the originating request;
- application EES resources remain separate from package preparation's private IMOS service;
- `eval` accepts only a `Value` export;
- `eval-with` enforces its context function type, source/env declarations, canonical source names
  and JSON/YAML/TOML data admission;
- the public actor protocol is exported entirely as first-order data plus the service reducer.

## Deferred work

This RFC does not add multiple Telora actors, actor addresses, actor-to-actor effects, actor fork,
migration, supervision, mailboxes, Highway, shared ring buffers, distributed execution or an
independent process/session lifecycle for each EES model.
