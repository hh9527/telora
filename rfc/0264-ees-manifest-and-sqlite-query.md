# RFC 0264: EES Manifests and SQLite Query Actors

- Status: Accepted
- Tracking: #146
- Depends on: RFC 0257, RFC 0263

## Summary

An Extra Effect Service is constructed from a complete typed manifest before
it admits requests. The manifest binds logical actor names to Native Actor
Component construction configuration. An operation can select an existing logical
actor but cannot create an actor or select a new physical resource.

Main modules declare complete EES actor configurations and constrained variables:

```telora
option "ees.vars" {"tenant": "[a-z][a-z0-9-]{0,31}"};
option "ees.sqlite" {
    name: "catalog",
    path: "user-data:catalog/{tenant}/db.sqlite",
};
```

Commands bind only declared variables with repeatable `--ees-var NAME=VALUE` arguments.

The first application component kinds are `imos` and `sqlite-query`. This
version adds one crate:

```text
crates/sqlite-query
```

An application may use an explicitly bound IMOS actor to materialize its own
plans. Telora package preparation separately constructs a private IMOS actor
before loading application code. The private actor is absent from the
application manifest, so application effects cannot address or interfere with
Telora package acquisition.

## Actor construction

`telora-ees` owns the strong construction types:

```text
Manifest {
    components: Array(ComponentSpec),
}

ComponentSpec =
    Imos {
        name: String,
        store: ResourceLocator,
        home: ResourceLocator,
    }
  | SqliteQuery {
        name: String,
        database: ResourceLocator,
    }

ResourceLocator = Physical(Path) | User(String)
```

Names are non-empty and unique within one Service. Construction validates all
specifications, resolves component-owned logical locators and opens all physical resources before returning. A failure is
a Host startup diagnostic, not a request event.

The first implementation uses a Rust enum rather than a generic public JSON
component schema. Two concrete components establish the reusable boundary
before a component SDK is extracted.

The lifecycle is:

```text
Manifest -> Service::open -> dispatch(Call) -> Reply
```

The Service owns every actor for its complete lifetime. A `serve` process can
therefore reuse IMOS state and SQLite connections across requests.

## Isolated service domains

Telora constructs separate Service values for separate authorities.

Package preparation creates a private service containing one reserved IMOS
actor:

```text
name  = telora-packages
store = Telora shared package cache
home  = <workspace>/.telora/crates-refs
```

These values are computed by the package Host. This service is used and
dropped before application execution. It is not included in application
`SystemCaps`, `SystemResources`, effects, events, options, or EES manifest.

Application execution creates another service solely from the selected Main's EES options and
validated `--ees-var` bindings.
It may contain an actor also backed by IMOS, but that actor has its own name,
store and home. Equal names in separate service domains do not grant access
across domains.

For example:

```text
telora serve app \
  --ees-var tenant=production \
  --bind stdio://
```

The Main declares IMOS construction fields independently:

```telora
option "ees.imos" {
    name: "a",
    home: "user-data:materialized/{tenant}",
    store: "user-cache:imos/{tenant}",
};
```

The CLI validates each variable against the full regular expression declared by `ees.vars`,
substitutes it as one safe path segment, and passes the resulting logical locator to the native
actor component adapter. The adapter resolves `user-data:`, `user-cache:`, `user-config:` and
`user-state:` through the corresponding XDG directory, with standard `$HOME`-based fallbacks.
Package preparation uses direct physical `store` and `home` paths and does not consume application
options or variables.

Actor names and kinds are visible to application capability validation.
Entry options retain logical locator templates, and `Env` contains only the name-to-kind map.
Resolved physical paths never enter a Telora World. Request-time component diagnostics identify
the logical actor; Host startup diagnostics may identify a resource that failed to open.

## IMOS actor operation

IMOS `store` and `home` are construction data. The operation is:

```text
InstallShared {
    plan: JsonValue,
}
```

The selected actor publishes the plan through its configured home and installs
through its configured store. The operation cannot choose either path.

Package acquisition calls this operation through a typed embedded Rust facade.
Application code calls the same component only through a logical actor present
in its own capability set. There is no application operation for discovering
another Service or its actors.

`telora ees` constructs an explicit service manifest from its CLI arguments;
it does not accept physical IMOS paths in individual requests.

## SQLite query component

`sqlite-query` owns one SQLite connection and query execution. It does not
depend on Telora, `telora-core`, Entry types, diagnostics, or the EES protocol.

Construction opens the database read-only. One actor serializes access to its
connection. Its operation is:

```text
Query {
    sql: String,
    bindings: Array(JsonScalar),
}
```

`JsonScalar` is Null, Bool, Int, Float or String. Bool binds as SQLite integer
zero or one. An unsigned integer outside SQLite `i64`, non-finite Float, Array,
Object or binary binding fails before execution.

The component accepts one read-only statement. It rejects writable statements,
multiple statements, DDL, transactions, attachment and write-affecting PRAGMA.
SQLite read-only open flags remain an independent write barrier.

Success is:

```text
QueryOutput {
    columns: Array(String),
    rows: Array(Array(JsonScalar)),
}
```

SQLite Null, Integer, Real and Text map to JSON scalars. Blob is unsupported.
Column count, row count and accumulated output bytes have fixed limits. A
limit failure returns no partial output.

## Application declarations

Main declares the complete logical actor set directly through component options:

```telora
option "ees.imos" {
    name: "a",
    home: "user-data:materialized/production",
    store: "user-cache:imos/production",
};
option "ees.sqlite" {name: "catalog", path: "user-data:catalog/production.sqlite"};
```

The EES-aware standard Entry checks exact equality between these declarations and Host bindings.
Duplicate names and malformed component records fail before Service construction. Unknown,
missing, duplicate, invalid or unused EES variables fail before component resource resolution.

`SystemCaps` carries the approved name-to-kind map. Before calling the Host,
the engine rejects an EES effect that names an actor outside the capabilities.
The Host repeats this check against its manifest and never routes by a physical
locator supplied in operation data.

## Generic Entry effect boundary

The private Entry runtime vocabulary adds one component-neutral call:

```text
EesCall = {
    key: String,
    actor: String,
    operation: String,
    input: Value,
}

SystemEffect += 'EesCall(EesCall)

EesReply = {
    key: String,
    result: Result(Value, String),
}

SystemEvent += 'EesReply(EesReply)
```

`key` correlates one asynchronous call with one reply. `actor` selects a
capability. `operation` and `input` are interpreted by `telora-ees`, which
validates them against the actor kind and adapts them to component-owned DTOs.

The Host converts semantic `Value` at the explicit effect boundary, dispatches
asynchronously, and queues exactly one reply event. Reducers remain
single-threaded and observe events one at a time. Component failure is a
request-local `Err(String)` and does not stop another actor or request.

There is no IMOS-specific or SQLite-specific variant in `SystemEffect`.
`telora-core` contains no native component dependency.

## Standard EES request surface

Synchronous native component calls inside VM evaluation would hide effects in
pure functions. `std/ees` therefore defines only a first-order request:

```telora
type Request = Tuple([String, String, Value]);
```

Component helper modules build ordinary calls. `std/sqlite-query` encodes
`{sql, bindings}` and returns the component's JSON-compatible `Value` reply;
`std/imos` encodes an `InstallShared` plan. Helpers do not execute effects.

`Request` is a transparent internal carrier. Applications construct it through
`ees.request` or component helpers rather than depending on tuple positions.

Applications expose one reducer service independently of which EES models they declare:

```telora
service: Fn(Dict(Value)) -> actor.Service
```

The reducer emits `actor.EesCall {id, request_id, request}` and receives a correlated
`actor.EesReply` event. Multi-step workflows keep their phase and correlation data in the explicit
application State. Independent requests may complete out of input order. Only translated
`SystemEffect` values cross into the Host.

## EES facade protocol

The EES facade uses one envelope:

```text
Call {
    id: String,
    actor: String,
    operation: String,
    input: JsonValue,
}
```

Success is a correlated JSON Value. Failure retains the request ID and a
concise message. Service-level protocol failures have an explicit null ID.
Request IDs are unique while in flight.

The facade validates operation names by actor kind:

```text
imos          -> InstallShared
sqlite-query  -> Query
```

An unknown operation, malformed input or actor mismatch is request-local. The
facade DTO does not expose `imos::Store`, rusqlite types, physical paths or
component-private reducer state.

## Dependency boundaries

```text
telora binary -> telora-ees -> imos
                           -> sqlite-query

telora-core   -> no EES or native component dependency
```

The CLI Host adapts transport-neutral Entry effect data to `telora-ees`.
Package Host also depends only on the EES facade.

## Verification

Tests establish:

- manifest construction rejects empty or duplicate names and invalid physical
  resources;
- IMOS requests cannot select `store` or `home`;
- package preparation uses a private Service absent from application caps;
- an application-bound IMOS actor installs only through its own root;
- SQLite binds scalars without interpolation and returns stable columns/rows;
- writable or multiple SQL statements, structured bindings, blobs and result
  limits fail without partial output;
- malformed declarations and variable bindings fail before Main initialization;
- generic EES effects cannot address undeclared actors or mismatch operations;
- `run` completes multiple sequential EES calls through explicit State;
- `serve` correlates concurrent calls and survives request-local failures;
- package acquisition, reducer run/serve, pure eval, check, query and LSP do not regress;
- dependency scanning confirms `telora-core` has no native component edge.

## Deferred work

This RFC does not add SQLite writes, migrations, transactions, connection
pools, blobs, named SQL parameters, dynamic actors, cross-Service discovery,
a generic component SDK, Highway transport or shared ring-buffer mailboxes.
