# RFC 0233: Pure Edge entries and CLI convergence

- Status: Implemented
- Depends on: RFC 0107, RFC 0161, RFC 0170 through RFC 0173, RFC 0227, RFC 0228, RFC 0231
- Tracking issue: #62
- Related issue: #55

## Summary

Telora converges its command-line Host surface around three user tasks and one
tool transport:

```text
telora run <binary-name> [-C <context>] [--input <file|->] [--entry <file>]
telora run -S <main-file> [--input <file|->] [--entry <file>]
telora check <module-id> [-C <context>]
telora show <module-id> [-C <context>] ...
telora lsp
```

`exec` and `build` are removed without compatibility aliases. They embedded two
domain-specific adapters in the language CLI. Exec plans, build plans, SQL
queries, and similar artifacts remain ordinary application values interpreted
by an explicitly authorized external Host.

`check`, `show`, and `lsp` retain their current fixed Host implementations and
select modules, not application entries. `run` selects a Main application and
an Edge entry. With no `--entry`, the Host uses the built-in run entry. With
`--entry <file>`, that explicitly selected user module receives Entry authority
and implements the same protocol in pure Telora.

An Edge entry has one language-level privilege: its imports may resolve any
graph-visible private or native module. It runs in WorkWorld, while the
application Main is initialized and frozen in MainWorld. Entry code remains
pure. It receives Host messages as `SystemEvent` values and returns
`SystemEffect` descriptions; only the Host performs effects.

## World model

The lifecycle has a preparation WorkWorld followed by a sequence of runtime
WorkWorlds:

```text
MainWorld::Building
    -> preparation WorkWorld
       entry.prepare(SystemOptions) -> SystemCaps
    -> discard preparation WorkWorld
    -> Host validates SystemCaps and prepares Main inputs
    -> Host loads and validates Main against entry.MainType
    -> MainWorld::Frozen
    -> runtime WorkWorld 0
       entry.initialize(main) -> (state, reduce)
       reduce(state, event) -> (next_state, effects)
    -> runtime WorkWorld 1
       reduce(next_state, event) -> ...
```

The preparation WorkWorld cannot publish a closure, heap identity, capability
handle, or provisional type into MainWorld. Its only accepted result is the
closed `SystemCaps` value. The Host validates that value before changing the
building MainWorld.

Main initialization is complete and authoritative before the first runtime
WorkWorld is created. Main code, imports, exports, and ordinary values are
frozen in MainWorld. Entry code, the reducer closure, current State, incoming
SystemEvent, and produced SystemEffect values remain in WorkWorld and are never
promoted into MainWorld.

At a completed reducer boundary, the runtime exports only SystemEffect values
to Host. It does not materialize State as an owned Host Value. Instead it traces
the next State root, preserves MainWorld edges, copies reachable Work objects
directly into a fresh WorkWorld with one forwarding table, and then discards
the preceding WorkWorld. Sharing, cycles, identity, and provenance survive
this relocation; unreachable reducer temporaries do not. A later optimization
may reuse one WorkWorld for multiple turns and perform the same copying
collection at a deterministic threshold, but that reuse is not part of this
RFC.

Calling a MainWorld closure from Entry executes ordinary Telora code in the
current WorkWorld over frozen Main roots. It does not let Main observe Entry,
State, system modules, or ambient Host authority.

## Selected Entry authority

The resolver already records one `selected_entry` and computes privilege from
requester identity. The Host assigns a reserved, non-importable identity to
either the embedded run Entry or the physical source named by `--entry`.
There is no public `entry/...` module namespace or general Entry loader.

For ordinary modules:

```text
visible = ordinary public modules allowed by the existing owner/dependency rules
```

For the exact selected Entry requester:

```text
visible = ordinary public modules
        + graph-visible *.priv.<format> modules
        + graph-visible *.native.telora modules
        + registered RuntimeSystem modules
        + the exact injected std/rt.priv.telora protocol module
```

This authority is non-transitive. An imported module retains its own identity
and ordinary resolver rules. A `.native.telora` source still needs a registered
Host implementation for every native declaration. Entry authority does not
load undeclared dependencies, escape a crate root, load arbitrary shared
libraries, or turn an ordinary file into native source.

The selected Entry has an identity distinct from every Main graph identity.
Importing the same physical source as an ordinary module cannot acquire the
Entry requester's authority. Main cannot import Entry, and Entry receives Main
as the argument of `initialize`, not through an ordinary application import.

## Entry ABI

Every user Entry exports exactly the following public protocol members:

```telora
import "std/rt.priv.telora" as rt;

export type MainType = ...;
export type State = ...;

export def prepare:
    Fn(rt.SystemOptions) -> rt.SystemCaps
    = ...;

export def initialize:
    Fn(MainType) ->
        Tuple([
            State,
            Fn(State, rt.SystemEvent) ->
                Tuple([State, Array(rt.SystemEffect)]),
        ])
    = ...;
```

Extra public exports are rejected so a misspelled protocol member cannot be
silently ignored. `MainType` and `State` must be authoritative concrete
TypeMetadata values. `prepare` and `initialize` must have the exact contracts
above; aliases and structurally equivalent types are accepted by ordinary
assignability.

`MainType` is not a System ABI type and has no global shape. Each Entry defines
the complete explicit export record it expects from its selected Main. The Host
loads Main only after `prepare`, validates that record against the selected
Entry's own `MainType`, and passes the checked record to `initialize`. A
mismatch fails before the runtime WorkWorld starts.

The built-in Entry independently chooses a dynamic Main boundary as its own
adapter policy. User entries normally express their expected Main record
statically. Neither choice changes `SystemEvent` or `SystemEffect`.

## Opaque State

State is private to Entry and opaque to Host. Its export exists only to prove
that the initial value, reducer input, and reducer output use one exact type.
Conceptually the Entry interface is an existential package:

```text
exists State.
    initialize: MainType ->
        (State, State * SystemEvent -> (State, Array(SystemEffect)))
```

The runtime roots the current State and passes that exact logical value to the
next reducer turn by direct WorkWorld relocation. Host must not receive an
owned representation of State, inspect fields, branch on its contents,
construct or modify it, compare it, encode it, persist it, publish it, or move
it into MainWorld.

## Pure event loop

Entry evaluation has no ambient IO. Its complete runtime transition is:

```text
State * SystemEvent -> State * Array(SystemEffect)
```

The Host serializes SystemEvent delivery. A reducer call completes atomically
before any returned effect is handled. If it fails, none of that call's
effects are visible to Host. Host processing never re-enters the reducer;
new events enter the queue for later turns.

SystemEffect has no synchronous return value. An external observation caused
by one effect may later arrive as an independent SystemEvent. Deterministic
replay fixes Main, the initial options, and the authored event order.

The Host executes effects on one asynchronous, concurrent event loop. Each
stdio child has an independently scheduled supervisor and independently
scheduled stdin, stdout, and stderr work. A blocked stdin write or stream read
therefore cannot block unrelated effects or event delivery. This contract
requires concurrency but makes no parallel-execution guarantee. Reducer calls
remain serialized: the Host injects one queued SystemEvent at a time and does
not invoke the reducer concurrently.

The initial protocol is intentionally narrow but includes stdio child process
orchestration. Its authoritative definitions live in `std/rt.priv.telora`:

```text
SystemOptions = { input: Option(Dyn) }
SystemCaps    = { input: Bool }

SystemEvent = Initialize
            | ChildStdout(ChildText)
            | ChildStderr(ChildText)
            | ChildSpawnResult(ChildSpawnResult)
            | ChildExited(ChildExited)

SystemEffect = SpawnStdioChild(SpawnStdioChild)
             | PostStdin(ChildText)
             | Exec(ChildOpts)
             | Output(String)
             | Exit(Int)
```

`input: 'True` requests that the CLI input value be installed as Main's
external `input` binding. The request fails when the CLI supplied no input.
`input: 'False` installs no binding. This ordinary record is a request, not an
authority token.

The Host starts with `Initialize`. `SpawnStdioChild` attempts to start one
process under a stable Entry-authored key and always schedules
`ChildSpawnResult { key, result: Result(Int, String) }`; an ordinary spawn
failure remains reducible Entry input rather than aborting the run Host.
`PostStdin` writes `Some(text)` or closes piped stdin with `None`. `PipedLine`
produces one `Some(line)` event per UTF-8 line without
its terminator; `PipedToEnd` produces at most one complete `Some(text)` at EOF.
Both modes then produce `None` as an explicit EOF event. `ChildExited` is sent
after both piped output streams reach EOF and distinguishes an exit code from
an optional terminating signal.

The Host owns process reaping. Every successfully spawned child is waited by
the Host: normal completion is reaped before `ChildExited`, while Entry Exit or
Exec, reducer failure, protocol failure, and Host stream failure terminate and
wait every remaining child. Entry code is never responsible for preventing
zombie processes. For `Exit(code)`, the terminal barrier stops children, waits
and reaps all of them, commits buffered Output, and only then permits the CLI to
call `std::process::exit(code)`. A wait failure prevents that requested status
from taking effect.

The Host owns all effect and child-supervision tasks in structured task sets.
Terminal completion, reducer failure, protocol failure, and Host failure first
signal cancellation and close child input mailboxes, then join every owned
task. A child supervisor likewise joins its stdin and stream tasks and waits
the child. Tasks may not be detached from these ownership trees.

`Output(String)` is an Entry-authored output effect, not a Main return type or
a Host serialization request. An Entry may encode any Main model with ordinary
Telora codecs, emit multiple ordered chunks, or emit output independently of
Main. The CLI buffers chunks until a terminal effect so a protocol failure does
not expose partial output. `Exit(Int)` terminates with that Host exit status.
`Exec(ChildOpts)` is terminal and replaces the Telora process on Hosts that
support process replacement. A terminal effect must be last; the Host stops
remaining stdio children before completing or replacing the process.

There is no internal wake event and no arbitrary turn bound. When the event
queue is empty, the Host waits for an active child observation. No queued event
and no active child is a deterministic no-progress error. Ordinary per-call VM
quotas still apply. Future HTTP, timer, filesystem, or network variants must
preserve pure Entry evaluation and explicit asynchronous event injection.

## Built-in run Entry

Omitting `--entry` selects an embedded Entry implementing current `run`:

1. request the external `input` binding exactly when `--input` is present;
2. accept Main through the built-in dynamic boundary;
3. on `Initialize`, select Main's explicit `output` export;
4. require that this particular adapter's selected value is `String`; and
5. return `Output(output)` followed by `Exit(0)`.

The String requirement belongs only to the built-in Entry. A user Entry chooses
its own `MainType` and output encoding and need not expose an `output` export.

`--entry` is a physical Host source selection, not a Main module import. The
CLI path is resolved from the process current directory and canonicalized; the
source must be a `.telora` file. The loaded source receives the reserved Entry
identity and uses the selected Main crate's manifest, stable module IDs, and
dependency graph. As with the current built-in Entry identity, it cannot use
relative, `@src`, `@bin`, or `@test` imports. Entry dependencies are named by
their stable dependency or registered module IDs. This avoids giving one
physical source a second ordinary graph identity merely to support Host
selection.

## Fixed tooling paths

`check`, `show`, and `lsp` remain fixed Host-owned command paths in this RFC.
They do not yet use the run Entry ABI, load a user Entry, or inspect a target
module for `prepare` or `initialize`.

`check` and `show` accept canonical module IDs. They inject no application
input, select no `output`, and interpret no application plan. `check` asks for
an authoritative module load and compile verdict. Recovery may continue work
to collect diagnostics, but cannot turn an error into success. It does not
claim that a later strict `run` will succeed. `show` builds the recoverable
semantic snapshot and emits the stable semantic query selected by its flags.
LSP is the long-lived transport over the same module-oriented workspace.

The exact diagnostic continuation, observation, and test-execution details of
CheckEntry remain tracked by #55. RFC 0233 removes the proposed need for a
separate domain command: strict executable acceptance belongs to `run`.

## Removed adapters

The following commands become unknown subcommands:

```text
telora exec
telora build
```

Their clap variants, Rust adapter functions, canonical plan serializers,
synthetic exec entry, CLI-only runtime bindings, and command fixtures are
removed. Reusable ordinary modules such as build-plan data constructors may
remain when they have independent library value. Historical RFCs continue to
describe the completed experiments but no longer define the current CLI.

## Unresolved issues

### Signal forwarding

The current run Host does not yet define explicit signal capture and
forwarding. OS default delivery is insufficient: a signal directed at the
Telora process may terminate it before the terminal barrier waits every child,
and forwarding only to direct child PIDs would not cover their descendants.

A follow-up must decide and specify at least:

- whether every spawned child owns a distinct process group and which signals
  the Host forwards to all active child groups;
- whether the first `SIGINT`, `SIGTERM`, or `SIGHUP` is both forwarded by the
  Host and exposed to Entry through a closed `SystemSignal` event;
- whether a second termination signal escalates to killing every child group,
  waiting every direct child, and exiting with `128 + signal`;
- the ordering of signal events relative to stdout, stderr, EOF, and
  `ChildExited` events;
- how `SIGWINCH`, job-control signals, and signals received during reducer
  evaluation behave; and
- how Windows Ctrl-C/Ctrl-Break maps to the same portable protocol without
  exposing invented Unix signal numbers.

Signal forwarding must preserve the existing terminal invariant: Entry
`Exit(code)` cannot cause `std::process::exit(code)` until the Host has waited
all successfully spawned direct children. No signal design may delegate child
reaping to Entry code.

## Non-goals

- arbitrary IO or ambient Host access from Entry evaluation;
- capability tokens, delegation, attenuation, or a general effect system;
- exposing private/native modules to Main;
- making Entry privilege transitive;
- persisting or serializing State;
- dynamic Main signatures computed by `prepare`;
- concurrent reducer calls or nondeterministic event delivery;
- HTTP, filesystem, network, timer, or service event/effect protocols; and
- compatibility aliases for `exec` or `build`.

## Acceptance criteria

1. CLI help exposes `run`, `check`, `show`, and `lsp`, but not `exec` or `build`;
2. omitted `--entry` preserves run, standalone, input, String output, debug,
   strict failure, and exit behavior through the built-in Entry;
3. `--entry <file>` selects a distinct Entry identity and uses the same
   prepare/freeze/initialize/reduce lifecycle;
4. only the exact Entry requester can cross dependency private/native
   visibility and resolve entry-only runtime modules;
5. Entry preparation returns only validated SystemCaps and cannot publish a
   WorkWorld value into MainWorld;
6. Main loads after preparation, receives only requested input, and must match
   the authoritative MainType before initialize runs;
7. State remains opaque to Host, never materializes as an owned Host Value,
   and moves directly between runtime WorkWorlds without entering MainWorld or
   publication;
8. reducer calls are pure and atomic with respect to Host-visible effects;
9. stdio child effects produce correlated spawned, text, EOF, and exit events;
10. Output is Entry-authored String data; malformed ABI, unauthorized caps,
    malformed effects, Host failure, and no progress commit no buffered output;
11. ordinary modules cannot import the private runtime surface or acquire
    Entry authority by importing the same physical source;
12. `check`, `show`, and LSP retain their module-oriented behavior;
13. LANGUAGE SSOT, CLI tutorial, README, and examples describe only the
    current command surface and authority boundary; and
14. formatting, complete tests, and warning-denied workspace Clippy pass.

## Implementation plan

1. assign a distinct reserved identity to the selected physical Entry source
   and use the existing selected-entry resolver context;
2. publish the narrow `std/rt.priv.telora` ABI and embedded built-in run Entry;
3. split entry preparation, Main initialization/freeze, and runtime loop into
   explicit Host phases while reusing PendingModule and WorkWorld machinery;
4. add `run --entry`, preserve default/standalone behavior, and cover authority
   and atomicity adversarially;
5. remove exec/build CLI adapters and their exclusive runtime surface;
6. define CheckEntry's verdict at the current module boundary and retain #55
   for richer diagnostic/test execution details;
7. update active design/tutorial documentation; and
8. run the full quality gate and record implementation evidence.

## Stopping rules

Implementation returns to discussion if it requires an effectful Telora VM,
ambient Host access, transitive Entry authority, State interpretation by Host,
partial effect publication from a failed reducer call, dynamic dependent Main
types, arbitrary module loading, or weakening MainWorld publication.

## Implementation evidence

- `telora run` now drives both the embedded and explicitly selected pure Entry
  through prepare, MainWorld initialization and freeze, and the event loop;
  `exec` and `build` are no longer CLI subcommands.
- the resolver grants private/native visibility only to the selected Entry and
  requires native modules to remain visible in the declared dependency graph;
- CLI and core tests cover the ABI, authority boundary, buffered Output,
  stdio child text/line/EOF/exit events, Exec, malformed effects, and
  no-progress handling; and
- `cargo fmt --all`, warning-denied workspace Clippy, and the complete workspace
  test suite pass.
