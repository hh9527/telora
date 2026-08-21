# RFC 0257: Configured Entry ABI and `run-with`

- Status: Accepted
- Supersedes: the Entry ABI and `run --entry` surface in RFC 0233
- Depends on: RFC 0233, RFC 0249

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
    Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);

export def config:
    Fn(rt.SystemOptions, rt.Env)
    -> Tuple([rt.SystemCaps, Initializer])
    = ...;
```

`SystemOptions` is the ordered sequence of immediate `option "..." ...;`
actions authored in the selected Main root. `Env` contains non-sensitive Host
facts and invocation arguments. `config` returns the exact capabilities needed
by this invocation and an initializer closure that captures all pure planning
results. The Host supplies a private native resource provider. After loading
Main normally, the engine invokes that provider inside the retained Entry
WorkWorld and passes its `SystemResources` result directly to the initializer;
the resource value never returns to Host orchestration.

The CLI surface is:

```text
telora run <binary> [run-options] [-- <entry-args>...]
telora run-with <entry-module> <binary> [run-options] [-- <entry-args>...]
```

`run` is exactly `run-with std/entry/default`. The old `run --entry <file>`
option is removed without compatibility behavior.

## Protocol types

The implemented protocol makes invocation arguments, requested Host resources,
and fulfilled resources explicit:

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
    platform: Platform,
};

type DataFormat = enum { 'Json, 'Yaml, 'Toml };
type DataSrc = struct {
    default: Option(Value), fmt: DataFormat, src: String,
};
type TextSrc = struct { default: Option(String), src: String };
type Stdin = enum { 'Text, 'Lined, 'Null };

type SystemCaps = struct {
    data_srcs: Dict(DataSrc),
    spawn_child: Bool,
    text_srcs: Dict(TextSrc),
    vars: Array(String),
    stdin: Stdin,
};

type SrcItem(T) = struct { data: T, src: String };
type SystemResources = struct {
    data: Dict(SrcItem(Value)),
    texts: Dict(SrcItem(String)),
    vars: Dict(String),
    stdin: Option(String),
};
```

`data_srcs` requests named JSON, YAML, or TOML sources. The private Host native
uses the same source-registration, CST, format-validation, and materialization
pipeline as JSON/YAML/TOML imports. Static imports materialize directly into the
building MainWorld; Entry resources materialize directly into the retained
Entry WorkWorld. Neither path constructs an intermediate `DataWorld`, decodes a
Telora value into a Host-owned representation, or copies a completed data graph
between Host and World. The private native also constructs the complete
`SystemResources` in that WorkWorld; `text_srcs` preserves both text and source
name.
If a requested data file is absent, `'Some(default)` supplies an already typed
`Value` while `'None` fails capability preparation. The default is not parsed
according to `fmt`. Text defaults remain strings. Other I/O failures always
fail. An existing data file that cannot be parsed never falls back to its
default.

The shared data-source pipeline has a strict phase boundary:

1. read the physical source while retaining its logical source name;
2. construct a lossless CST and validate all format-level rules without
   allocating runtime data objects;
3. only after validation succeeds, materialize the generic `Value` graph
   directly into the target Heap.

Format-level validation includes every content condition that could otherwise
make materialization fail, such as duplicate object keys, TOML table conflicts,
invalid numeric or temporal values, and unsupported or ambiguous YAML graph
features. Failure registers a sourced diagnostic and produces no runtime
`Value`. This layer does not check application schemas: both static data imports
and `data_srcs` deliberately expose generic `Value`; business conformance is a
separate codec concern.

Validation is a borrowed view over the lossless CST, not a second owned data
tree. Validators retain node IDs, spans, and references into source/CST storage;
they do not copy scalar text or normalize complete arrays and objects into an
intermediate graph. Small side tables are allowed only where required for
duplicate detection, YAML graph resolution, TOML table assembly, or a
deterministic materialization order. Their entries should identify CST nodes
rather than own duplicated payloads. A validated source is therefore a compact
plan of references that can be consumed once by the target-Heap materializer.

Materialization does not consume or depend on Telora VM fuel, stack, or Heap
allocation quota. Raw sources are read with a `file_size` bound before a complete
source String is allocated. The validated logical graph is admitted with
independent limits for total nodes, root-based depth, per-container items,
per-Bytes length, per-String UTF-8 length, and total decoded payload bytes.
Object keys contribute String and payload bytes but are not nodes; YAML aliases
and merges contribute once per final logical occurrence. All accounting uses
checked arithmetic. Every materialized node retains the `SourceId` and range
allocated by the same run's source registry, so later diagnostics may use the
data node as its source location.

Once the dependency graph and stable module slots exist, a validated static
data module may allocate its `Value` and export module directly in the unsealed
MainWorld. Entry resources use the same materializer with the Entry WorkWorld
as target and the MainWorld as background. The target differs; the validation,
location, data-limit, and `Value` construction rules do not.

`vars` is a requested snapshot, not a list of required variables. Missing
names are omitted from `SystemResources.vars`; values present in the process
environment but not representable as strings fail capability preparation.

`stdin: 'Text` reads stdin to EOF and places it in `SystemResources.stdin`.
`'Lined` leaves the resources field empty and emits
`'StdinLine('Some(line))` events followed by exactly one
`'StdinLine('None)` at EOF. `'Initialize` always precedes these events.
`'Null` neither injects stdin nor emits stdin events.

`spawn_child` explicitly authorizes both `'SpawnStdioChild` and `'PostStdin`.
Before executing any effect in a reducer result, the Host audits the complete
batch and rejects either effect when this capability is false. Existing
`'Exec` authority is separate from `spawn_child`.

Repeated Main options preserve authored order. Option values remain immediate
Telora values, wrapped as `Dyn` only at the heterogeneous Entry boundary. Main
options are extracted before Main evaluation, but this RFC does not introduce
dynamic or staged Main loading. Main top-level evaluation cannot observe the
resources. An application that depends on invocation context exposes that
dependency explicitly, for example `main: Fn(Ctx) -> Plan`, and the Entry calls
it with a context built from `SystemResources` and captured `Env`.

## Host lifecycle

The Host performs one deterministic lifecycle:

1. resolve the Main root and selected Entry;
2. extract the Main root's ordered option actions without evaluating Main;
3. load Entry and validate its exact `MainType`, `State`, and `config` exports;
4. call `config(options, env)` and retain the returned initializer closure;
5. validate `SystemCaps` and configure event-producing Host capabilities;
6. load and evaluate Main once, then check its complete export record against
   `MainType`;
7. in one VM runner call, invoke the private resource native and pass its
   `SystemResources` result directly to `initializer(resources, main)`;
8. inject `Initialize` and run the existing serial reducer/effect loop.

The initializer closure stays in the retained Entry WorkWorld while the Host
configures event sources and Main is loaded. `SystemResources` is created in
that same WorkWorld and remains a VM register/heap value between the private
native and initializer. It is never decoded, mirrored, or rebuilt as a
Host-owned Telora value. Existing WorkWorld/MainWorld relocation preserves
closure, resources, Main, state, identity, and provenance.

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

Workspace and standalone selection, `-C`, `-S`, and `--best-effort` retain
their existing meaning. Arguments after `--` are not
Telora tool options; the Host copies them verbatim into `Env.args` for the
Entry to interpret.

There is no `--input` option. Entry owns argument parsing and expresses all
resource requests through `SystemCaps`.

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
   add the option, environment, caps, and resources protocol values.
2. Extract ordered Main options before Main evaluation, call `config`, retain
   its initializer closure, prepare requested resources, and call it with Main.
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
  and platform facts.
- requested data, text, variables, and text stdin are delivered only through
  `SystemResources`; absent variables are omitted, absent files use explicit
  defaults, and unavailable files without defaults fail before initialization.
- existing malformed data files fail instead of falling back to defaults.
- lined stdin emits Initialize, lines, and exactly one EOF event in order.
- child-process effects require `spawn_child`, and rejection commits no effect
  from the invalid batch.
- the initializer closure can capture values computed by `config`.
- ordinary imports and root selection of `.entry.telora` fail at resolution.
- `run --entry` is rejected by clap.
- existing child-process, effect trust, terminal barrier, best-effort, and
  MainType regressions remain green under the new ABI.
