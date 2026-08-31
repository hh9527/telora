# RFC 0264: EES Manifests and SQLite Query Actors

- Status: Accepted
- Tracking: #146
- Depends on: RFC 0257, RFC 0263

## Summary

An Extra Effect Service is constructed from a complete, typed manifest before
it admits requests. The manifest binds logical actor names to physical native
component configuration. Requests select an existing logical actor and carry
only operation data.

This version adds the second Native Actor Component, `sqlite-query`, and one
new crate:

```text
crates/sqlite-query
```

It also moves IMOS `store` and `home` from operation data into actor
construction data. Telora package preparation privately constructs and calls
the IMOS actor before loading application code. Application Telora cannot
submit IMOS operations.

`telora run` and `telora serve` accept repeatable database bindings:

```text
--db NAME=sqlite://PATH
```

The Host uses these bindings to construct application SQLite query actors.
Application requests refer only to `NAME` and contain SQL plus positional
bindings. Physical database paths never enter a Telora World.

## Manifest construction

`telora-ees` owns a strongly typed construction manifest:

```text
Manifest {
    components: Array(ComponentSpec),
}

ComponentSpec =
    Imos {
        name: String,
        store: Path,
        home: Path,
    }
  | SqliteQuery {
        name: String,
        database: Path,
    }
```

Component names are non-empty and unique within one service. Construction
validates every component and opens all required physical resources before
returning a Service. A construction failure is a Host startup failure, not a
request event.

The first implementation uses a Rust enum rather than a public generic JSON
component schema. This keeps component construction typed while two concrete
components establish the reusable boundary. It does not introduce dynamic
plugins, a component SDK, or a stable Rust ABI.

The lifecycle is:

```text
Manifest -> Service::open -> dispatch(Request) -> TerminalEvent
```

An operation cannot add actors, replace actor configuration, or select a new
physical resource.

## Internal package IMOS actor

Package preparation constructs one reserved IMOS actor from Host-owned values:

```text
name  = telora-packages
store = Telora shared package cache
home  = <workspace>/.telora/crates-refs
```

The package Host computes these values. They are not read from the application
Main module and are not included in application `SystemCaps`, `SystemEffect`,
`SystemEvent`, or `SystemResources`.

The IMOS operation becomes:

```text
InstallShared {
    id: String,
    actor: String,
    plan: JsonValue,
}
```

`home` is no longer request data. The selected actor publishes every submitted
plan through its configured home and installs through its configured shared
store. Package acquisition uses a typed embedded Rust call. `telora ees` may
expose the same operation for service-level integration, but this transport is
not an application capability.

The standalone EES CLI supplies both IMOS construction parameters:

```text
telora ees --store PATH --home PATH
```

All requests admitted by that process select the preconstructed IMOS actor.

## SQLite query component

`sqlite-query` owns a SQLite connection and query execution. It does not depend
on Telora, `telora-core`, Entry types, diagnostics, or the EES protocol.

Construction opens the configured database read-only. One actor serializes
operations through its connection. A `serve` Host retains the actor for the
whole service lifetime, allowing SQLite and the component to reuse connection
state and prepared resources.

The component request is:

```text
Query {
    sql: String,
    bindings: Array(JsonScalar),
}
```

`JsonScalar` is Null, Bool, Int, Float, or String. Bool binds as SQLite integer
zero or one. An unsigned JSON integer that does not fit SQLite `i64`, a
non-finite float, Array, Object, or binary value is rejected before execution.

The first version accepts exactly one read-only SQLite statement. Prepared
statements classified as writable are rejected. Multiple statements, DDL,
transactions, attachment and write-affecting PRAGMA operations are rejected.
The database is also opened with SQLite read-only flags so validation is not
the only write barrier.

Success is:

```text
QueryOutput {
    columns: Array(String),
    rows: Array(Array(JsonScalar)),
}
```

SQLite Null, Integer, Real and Text cells map directly to JSON scalar values.
Blob cells are unsupported in this version and fail the request. Column count,
row count and accumulated output bytes have fixed admission limits. Crossing a
limit fails the request without returning a partial result.

## Application database bindings

Application commands accept:

```text
telora run BINARY --db NAME=sqlite://PATH
telora serve BINARY --db NAME=sqlite://PATH --bind stdio://
```

`--db` is repeatable. Names must be non-empty and unique. The first version
accepts only `sqlite://`; it treats the remainder as a filesystem path and
requires a non-empty path. The Host canonicalizes its internal locator but
publishes only this logical source name:

```text
@run-ctx/db/<percent-encoded-name>
```

Main declares its complete database requirement once:

```telora
option "run-ctx.databases" ["a", "analytics"];
```

The database-aware standard Entry checks exact set equality between declared
and provided names. Missing, extra and duplicate names fail during Entry
configuration. `SystemCaps` carries the approved logical names. The Host
rejects any query effect whose database is outside those capabilities even if
an actor with that name exists.

## Entry query ABI

The private Entry runtime vocabulary adds:

```text
SqliteQuery = {
    key: String,
    db: String,
    sql: String,
    bindings: Array(Value),
}

SystemEffect += 'SqliteQuery(SqliteQuery)

SqliteQueryResult = {
    key: String,
    result: Result(Value, String),
}

SystemEvent += 'SqliteQueryResult(SqliteQueryResult)
```

`key` correlates an asynchronous effect with its result. The engine validates
the effect shape and capability before calling the Host. The Host converts
Telora `Value` scalars to the EES request, dispatches it asynchronously, and
queues exactly one result event. Reducers remain single-threaded and observe
events one at a time.

The Entry ABI contains no IMOS effect or event.

## Standard task surface

Synchronous native SQLite calls inside VM evaluation would hide an external
effect inside a pure Telora function. Instead, `std/sqlite` defines an explicit
continuation task:

```telora
type Query = struct {
    db: String,
    sql: String,
    bindings: Array(Value),
};

type QueryOutput = struct {
    columns: Array(String),
    rows: Array(Array(Value)),
};

type Task = enum {
    'Done(Value),
    'Query(struct {
        request: Query,
        then: Fn(Result(QueryOutput, String)) -> Task,
    }),
};
```

When no `--db` is provided, `run` and `serve` retain their existing pure Main
contracts. When at least one database is provided, the CLI selects
database-aware standard Entries with these contracts:

```telora
run:   main: Fn(Dict(Value)) -> sqlite.Task
serve: serve: Fn(Dict(Value)) -> Fn(Value) -> sqlite.Task
```

The run Entry drives one task until `Done`, JSON-encodes the value and exits.
The serve Entry starts one task for every input request, correlates pending
continuations by generated effect key, and emits each completed response using
the existing in-band diagnostic envelope. Independent requests may complete
out of input order. Each task is sequential unless it explicitly produces a
later Query from its continuation.

This is ordinary interpreted Telora code. The Host executes only explicit
`SystemEffect` values. No database handle, native closure or synchronous I/O is
inserted into Main arguments.

## EES request and terminal events

The EES facade adds an application operation:

```text
SqliteQuery {
    id: String,
    db: String,
    sql: String,
    bindings: Array(JsonScalar),
}
```

Success retains the request ID and carries `QueryOutput`. Failure retains the
request ID and a concise component error. Service-level protocol errors use an
explicit null ID. IMOS and SQLite terminal results are distinct typed variants
inside Rust; their JSON encoding remains unambiguous.

Request IDs remain unique while in flight. SQLite failures are request-local
and do not stop the service or another database actor.

## Dependency boundaries

```text
telora binary -> telora-ees -> imos
                           -> sqlite-query

telora-core   -> no EES or native component dependency
```

`telora-core` defines only transport-neutral Host capability, effect and event
data. It does not open SQLite, construct EES requests, depend on rusqlite or
know an IMOS type. The CLI Host adapts these values to `telora-ees`.

## Verification

Tests establish:

- IMOS uses manifest `store/home` and `InstallShared` cannot select a home;
- package preparation privately uses the reserved package actor;
- a SQLite actor opens read-only, binds every scalar without interpolation,
  and returns stable columns and rows;
- writable and multi-statement SQL, unsupported bindings, blobs and result
  limits fail without partial output;
- duplicate manifest names and missing physical resources fail construction;
- undeclared, missing, extra and duplicate CLI database names are rejected;
- `run --db` completes a chained query task;
- `serve --db` processes multiple requests, correlates results and survives a
  request-local SQL failure;
- an Entry cannot emit an IMOS operation;
- package acquisition, pure run/serve, check, query and LSP do not regress;
- dependency scanning confirms `telora-core` has no native component edge.

## Deferred work

This RFC does not add SQLite writes, execute, migrations, transactions,
connection pools, user-selected open flags, blobs, named SQL parameters,
dynamic actors, application package installation, a generic component SDK,
Highway transport or shared ring-buffer mailboxes.
