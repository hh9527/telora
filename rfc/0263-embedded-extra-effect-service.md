# RFC 0263: Embedded Extra Effect Service

- Status: Accepted
- Tracking: #145
- Depends on: RFC 0262

RFC 0264 extends this boundary with typed manifests, application-visible EES
tasks, isolated package/application Services and the `sqlite-query` component.
Its accepted definitions govern those surfaces.

## Summary

Telora ships one binary and embeds native effect capabilities behind one
`telora-ees` facade. EES means Extra Effect Service. The first Native Actor
Component is IMOS, which provides shared immutable installation. Its public
operation is named `InstallShared`.

The workspace adds exactly two crates in this version:

```text
crates/telora-ees
crates/imos
```

The dependency direction is:

```text
telora binary -> telora-ees -> imos
telora-core                    (does not depend on imos)
```

`telora` never calls an IMOS type or store directly. `telora-ees` owns the
facade request and event types, composes the native component, and adapts those
types to IMOS internals. `telora ees` is a thin CLI and JSON Lines transport
wrapper around the same embedded service used by workspace package
acquisition.

This RFC establishes a small executable boundary. It does not introduce the
Actor Highway, ring-buffer mailboxes, dynamic components, or a general native
component SDK.

## Motivation

RFC 0262 makes IMOS the owner of remote package download, unpacking, immutable
publication, reuse and collection. Its first implementation locates an
external `imos` executable and invokes `imos create` once for each remote
package plan. This has three undesirable consequences:

- installing Telora is insufficient to use Telora workspaces with remote
  packages;
- the package Host knows an IMOS command-line contract rather than one native
  effect facade;
- Telora and IMOS cannot evolve their request protocol atomically.

IMOS already implements its installation and status pipelines as deterministic
reducers that produce asynchronous effects. It is therefore useful as the
first concrete Native Actor Component rather than as an incidental external
program.

The merge is deliberately asymmetric. Telora does not absorb IMOS internals.
It depends on `telora-ees`, while `telora-ees` is the composition root for IMOS
and future native effect components.

## Workspace and binary layout

The existing IMOS library source, tests and required license attribution move
into `crates/imos`. The crate keeps its package name `imos`. It has no binary
target after the migration.

`crates/telora-ees` is a library crate. It owns:

- the EES public request and event data types;
- request-ID admission and in-flight tracking;
- request dispatch to composed Native Actor Components;
- terminal completion and failure event construction;
- optional progress event routing;
- the JSON Lines service adapter used by `telora ees`;
- embedded client conveniences that still use the same request dispatch.

The `telora` binary adds an `ees` subcommand and otherwise consumes only the
`telora-ees` facade. No separate `imos` executable is built or published.

This version does not add `telora-ees-core`, `ees-protocol`, a component SDK,
or a dynamic plugin crate. A reusable component abstraction may be extracted
after a second Native Actor Component provides concrete composition pressure.

## EES operation

The first public request is:

```text
InstallShared {
    id: String,
    home: Path,
    plan: JsonValue,
}
```

`InstallShared` means that the component:

- persists the submitted immutable plan under its request home;
- derives stable request, download and installation keys;
- merges concurrent work for equal keys across callers and processes;
- constructs an immutable installation root;
- atomically publishes and reuses that root;
- returns the root selected by the plan.

It is not a general operation that installs mutable files into an arbitrary
prefix. The previous public request name `Install` is removed rather than kept
as an alias.

This rename applies to the EES request and its public operation/status
vocabulary. IMOS-private reducer events and plan item names such as
`InstallFile` and `InstallBin` may retain their precise internal names; they
are not part of the EES facade.

The public DTO is defined by `telora-ees`. It must not contain `imos::Store`,
IMOS lock types, progress handles, internal prepared plans, or any other IMOS
type. The IMOS adapter receives EES-owned data and converts it at the component
boundary.

## Rust facade

The exact Rust spelling may follow implementation needs, but the facade has
these semantic operations:

```text
Service::open(Config) -> Service
Service::submit(Request) -> stream of Event
```

Every accepted request has one non-empty ID. IDs must be unique among requests
currently in flight in one Service. An accepted request produces zero or more
progress events followed by exactly one terminal completion or failure event.
The terminal event retains the request ID.

An embedded convenience may await the terminal event and expose an ordinary
Rust `Result` to package acquisition. It is an EES client layered over
`Service::submit`; it must not call `imos::Store` directly or construct a
second installation path.

The EES Rust API is not a stable dynamic-library ABI. The stable boundary in
this RFC is the request/event semantics and JSON encoding. Rust crates in the
monorepo may evolve atomically.

## JSON Lines transport

`telora ees` starts one EES Service, reads requests from stdin and writes
terminal events to stdout. The first-version CLI is:

```text
telora ees [--store <path>] [-e|--events-to-stderr]
```

It does not discover or prepare a Telora workspace and does not load Telora
source. `--store` selects the IMOS component store; omission uses the component
default. Store selection is physical Host configuration and does not enter a
plan identity or returned logical provenance.

Each non-blank stdin line contains one complete JSON request:

```json
{"type":"InstallShared","id":"request-42","home":"/refs","plan":{}}
```

Successful completion is:

```json
{"id":"request-42","type":"result","root":"/store/install/key/root"}
```

Request-local failure is:

```json
{"id":"request-42","type":"error","message":"..."}
```

The adapter preserves the established service rules:

- stdout contains JSON Lines protocol events and no human-oriented text;
- accepted requests may execute concurrently and complete out of order;
- each output stream has one serialized writer and bounded backpressure;
- request-local component failure does not stop other requests;
- duplicate in-flight IDs, invalid request envelopes and malformed JSON are
  protocol errors;
- stdin EOF stops admission, waits for accepted requests to reach terminal
  output, flushes output and exits;
- stdout closure or write failure terminates the service and cancels owned
  work;
- `--events-to-stderr` places non-terminal progress and recoverable diagnostics
  on stderr as JSON Lines; otherwise stderr remains empty during successful
  service operation.

The implementation reuses one EES reducer/dispatch path for embedded and
JSONL callers. The JSONL adapter performs decoding and output effects; it does
not implement shared installation decisions.

## Embedded package acquisition

Workspace package preparation still occurs before module graph discovery,
Entry selection, Main loading or VM evaluation. The package Host:

1. discovers and validates the workspace configuration and lock;
2. generates deterministic IMOS plans under `.telora/crates-refs/`;
3. constructs an EES `InstallShared` request for each required remote crate;
4. submits it to the embedded `telora-ees` Service;
5. awaits its terminal event;
6. validates the returned immutable crate root;
7. gives the complete `crate-name -> root` map to the resolver.

The embedded path does not serialize through stdin/stdout and does not spawn
the current executable. It nevertheless uses the same EES Request, request
admission, dispatch, IMOS adapter and terminal Event semantics as `telora ees`.

The following integration boundary is removed:

```text
TELORA_IMOS
Command::new("imos")
imos create <plan-file>
```

A clean installation containing only the `telora` executable is sufficient to
materialize a remote package workspace.

## Component ownership

`imos` continues to own:

- plan validation and durable request publication;
- content-addressed download and installation keys;
- file locks and cross-process work merging;
- download, digest verification, unpacking and artifact installation;
- temporary roots, atomic immutable publication and reuse;
- component progress reduction;
- removal and garbage-collection primitives retained by its library.

`telora-ees` owns:

- the public `InstallShared` request name and shape;
- service-level request admission and active-ID state;
- component invocation as an EES effect;
- conversion of component completion into EES events;
- terminal/progress output effects for the JSONL transport;
- native component construction and configuration.

`telora` owns:

- CLI parsing and selection of `telora ees`;
- workspace package-plan construction;
- consumption of the EES facade;
- validation of the crate root returned to package resolution.

`telora-core` owns no EES or IMOS dependency. Module resolution and VM
evaluation continue to receive already-materialized roots.

## Failure and lifecycle

The EES service reducer is the only owner of its request lifecycle state.
Component effects cannot directly remove active request IDs. An ID becomes
reusable only after its terminal output effect succeeds and the corresponding
completion event returns to the reducer.

Effect tasks may use immutable environment/context capabilities but must not
borrow or mutate reducer State. Component success, component failure, output
success and output failure all return as events. Join results only recover
runtime resources and detect task failure; they do not bypass the reducer to
advance request state.

The embedded client may project a terminal event into `Result`, but that
projection does not weaken the underlying lifecycle.

## Migration

- Move the IMOS library and tests into `crates/imos` without rewriting its
  installation algorithm.
- Move the outer JSONL request server/facade responsibility from IMOS into
  `crates/telora-ees`.
- Rename the public EES operation and associated protocol tests from `Install`
  to `InstallShared`.
- Add `crates/telora-ees` and `crates/imos` to the existing `crates/*` Cargo
  workspace.
- Add `telora ees` and delegate it to `telora-ees`.
- Replace package Host process invocation with the embedded EES client.
- Remove external-IMOS fixtures and replace them with EES facade and complete
  package-acquisition tests.
- Refresh RFC 0262 and the LANGUAGE, CONCEPT, IMPLEMENTATION and README SSOT to
  describe the embedded EES boundary positively.

The original IMOS Git history remains available in its source repository. The
monorepo migration preserves copyright and license information required by its
MIT license.

## Deferred work

This RFC does not define:

- shared ring-buffer storage or the Actor Highway protocol;
- Actor identity, mailbox routing or lifecycle effects;
- serialization between in-process Telora Actors;
- SQLite, HTTP, event-stream, IPC or child-process components;
- a reusable Rust reducer/effect framework extracted from IMOS;
- a general component trait, registry or dynamic component discovery;
- dynamic libraries or a stable Rust ABI;
- a Telora Entry `SystemEffect::InstallShared`;
- exposure of EES capabilities to ordinary Telora modules;
- changes to package identity, lock documents or resolver semantics.

These are follow-up decisions informed by the executable EES/component
boundary established here.

## Acceptance criteria

- The Cargo workspace contains exactly the two new crates `telora-ees` and
  `imos` for this feature.
- The repository builds and publishes one `telora` executable and no `imos`
  executable.
- `telora`, its package Host and `telora-core` contain no direct dependency on
  IMOS internals; only `telora-ees` composes the `imos` crate.
- `telora ees` accepts multiple JSONL `InstallShared` requests and produces
  correlated terminal events with the documented failure isolation.
- Equal plan/download keys continue to share physical work and immutable
  results across concurrent callers and processes.
- `telora lock`, `check`, `run`, `serve`, `query` and LSP materialize required
  remote crates without an external executable or `TELORA_IMOS`.
- Local-only workspaces and standalone commands do not initialize the shared
  installer unnecessarily.
- Package lock, crate identity, canonical module identity and provenance remain
  independent of the physical EES store path.
- EES reducer tests prove that an active ID is released only after terminal
  output succeeds.
- Existing IMOS store/reducer/effect tests remain effective after migration.
- Formatting, workspace tests, warning-denied Clippy and source-size checks
  pass.
