# RFC 0257: Configured Entry ABI and `run-with`

- Status: Accepted
- Supersedes: the Entry ABI and `run --entry` surface in RFC 0235
- Depends on: RFC 0235, RFC 0236, RFC 0237, RFC 0249

## Summary

Telora makes the selected Entry the only adapter between a Main application and
the Host. An Entry is a dedicated `.entry.telora` module and implements one
staged ABI:

```telora
export type MainType = ...;
export type State = ...;

type Transition = Tuple([State, Array(rt.SystemEffect)]);
type Reducer = Fn(State, rt.SystemEvent) -> Transition;
type Initializer =
    Fn(rt.SystemInjection, MainType) -> Tuple([State, Reducer]);

export def config:
    Fn(rt.SystemOptions, rt.Env)
    -> Tuple([rt.SystemCaps, Initializer])
    = ...;
```

`SystemOptions` is the ordered sequence of immediate `option "..." ...;`
actions authored in the selected Main root. `Env` contains non-sensitive Host
facts and invocation arguments. `config` returns the exact capabilities needed
by this invocation and an initializer closure that captures all pure planning
results. The Host prepares an immutable `SystemInjection`, loads Main normally,
and calls the initializer with the injection and Main export record.

The CLI surface is:

```text
telora run <binary> [run-options] [-- <entry-args>...]
telora run-with <entry-module> <binary> [run-options] [-- <entry-args>...]
```

`run` is exactly `run-with std/entry/default`. The old `run --entry <file>`
option is removed without compatibility behavior.

## Protocol types

The first implemented protocol keeps the existing external input capability:

```telora
type OptionAction = struct {
    key: String,
    value: Dyn,
};

type SystemOptions = Array(OptionAction);

type Platform = struct {
    os: String,
    arch: String,
};

type Env = struct {
    args: Array(String),
    input: Bool,
    platform: Platform,
};

type SystemCaps = struct {
    input: Bool,
};

type SystemInjection = struct {
    input: Option(Dyn),
};
```

`Env.input` reports whether the invocation supplied `--input`; it does not
expose the value. The Entry requests that value with `SystemCaps.input`. The
Host rejects a requested but unavailable input and passes the value only
through `SystemInjection.input`. Later RFCs may add environment snapshots,
source-backed files, or stdio event sources by extending the caps/injection
protocol; they do not grant ordinary Main modules ambient Host access.

Repeated Main options preserve authored order. Option values remain immediate
Telora values, wrapped as `Dyn` only at the heterogeneous Entry boundary. Main
options are extracted before Main evaluation, but this RFC does not introduce
dynamic or staged Main loading. Main top-level evaluation cannot observe the
injection. An application that depends on invocation context exposes that
dependency explicitly, for example `main: Fn(Ctx) -> Plan`, and the Entry calls
it with a context built from `SystemInjection` and captured `Env`.

## Host lifecycle

The Host performs one deterministic lifecycle:

1. resolve the Main root and selected Entry;
2. extract the Main root's ordered option actions without evaluating Main;
3. load Entry and validate its exact `MainType`, `State`, and `config` exports;
4. call `config(options, env)` and retain the returned initializer closure;
5. validate `SystemCaps` and prepare `SystemInjection`;
6. load and evaluate Main once, then check its complete export record against
   `MainType`;
7. call `initializer(injection, main)`;
8. inject `Initialize` and run the existing serial reducer/effect loop.

The initializer closure stays in the retained Entry WorkWorld while the Host
prepares the injection and Main. No arbitrary Host-owned Telora value is
materialized. Existing WorkWorld/MainWorld relocation preserves closure,
injection, Main, state, identity, and provenance.

`config`, initializer, and reducer failures are Entry failures and remain
outside the Entry program. The existing effect trust audit remains unchanged:
an entire effect batch is checked before the Host executes its first effect.

## Entry module identity and visibility

Every source Entry ends in `.entry.telora`. This suffix is a language-level
module category, like `.priv.telora` and `.native.telora`, rather than a naming
convention.

- an ordinary module cannot import a `.entry.telora` module;
- a `.entry.telora` module cannot be selected as Main or standalone root;
- `run-with` is the only ordinary CLI operation that selects an Entry;
- the selected Entry retains the existing authority to import private/native
  modules in the resolved dependency graph;
- Entry authority does not propagate to modules imported by the Entry.

The built-in default source is `std/entry/default.entry.telora`; the CLI uses
the stable selector `std/entry/default`. Custom selectors are canonical module
IDs whose resolved physical source has the `.entry.telora` suffix.

This suffix prevents Main or a dependency from acquiring Entry protocol values
or privileged helpers merely by importing an Entry implementation. Host
effects are still authorized structurally: the Host interprets caps and
effects only from the selected Entry boundary.

## CLI behavior

`run-with` takes the Entry selector before the binary name:

```text
telora run-with std/entry/default main
telora run-with @src/serve.entry.telora main
```

Workspace and standalone selection, `-C`, `-S`, `--input`, and
`--best-effort` retain their existing meaning. Arguments after `--` are not
Telora tool options; the Host copies them verbatim into `Env.args` for the
Entry to interpret.

`run` and explicit default `run-with` must have identical loading, diagnostics,
output, termination, and best-effort behavior.

## Deferred work

This RFC does not add:

- `with_diagnostics` or request-local diagnostic capture;
- stdio JSONL serving events/effects;
- TCP, Unix-domain sockets, HTTP, or streaming output;
- `load_main().await` or arbitrary dynamic module loading;
- synthetic `rt/env` modules or config-dependent module-map mutation; or
- direct privileged file/environment functions.

These features can build on the configured Entry ABI without changing its
staging boundary.

## Implementation plan

1. Replace `prepare`/`initialize` exports with the staged `config` export and
   add the option, environment, caps, and injection protocol values.
2. Extract ordered Main options before Main evaluation, call `config`, retain
   its initializer closure, prepare input injection, and call it with Main.
3. move the built-in Entry to `std/entry/default.entry.telora` and update its
   source and ABI.
4. add `run-with`, define `run` as default selection, remove `run --entry`, and
   pass trailing Entry arguments through `Env.args`.
5. make `.entry.telora` unimportable and invalid as an ordinary root while
   allowing explicit `run-with` selection.
6. migrate Entry tests, SSOT, guides, and CLI help; run the complete regression
   suite.

## Acceptance criteria

- `telora run main` and `telora run-with std/entry/default main` are
  behaviorally identical.
- a custom `.entry.telora` module receives ordered Main options, Entry args,
  platform facts, and the input availability bit.
- requested input is delivered only through `SystemInjection`; unavailable
  requested input fails before initializer invocation.
- the initializer closure can capture values computed by `config`.
- ordinary imports and root selection of `.entry.telora` fail at resolution.
- `run --entry` is rejected by clap.
- existing child-process, effect trust, terminal barrier, best-effort, and
  MainType regressions remain green under the new ABI.
