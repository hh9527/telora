# Telora

> **TELORA Enables Lowering Objectives to Reliable Artifacts.**
>
> Telora was formerly known as Forma and was originally called XL. Its design
> history is recorded in [rfc/](rfc/).

**Telora is an experimental language for programmable data transformation and
validation in a closed, pure, deterministic, and source-aware world.**

It is designed as a verified intent language between agents and the real
world: programs express objectives, libraries validate and lower them, and
hosts decide whether the resulting artifacts may affect external systems.

It asks:

> What is the smallest language that can provide general data computation,
> finite execution boundaries, and first-class diagnostics and feedback?

Telora sits between static configuration and general-purpose scripting. Static
formats are inspectable but limited; scripting languages are programmable but
often open, effectful, difficult to reproduce, and weak at explaining the
origin of transformed data. A sandbox with fuel and an API allowlist can bound
a script, but it does not by itself provide an authoritative semantic model,
cross-data provenance, recoverable analysis, or precise editor feedback.

Telora treats those requirements as one design problem.

## The Core Model

### Ordinary computation over ordinary data

Configuration, validation, normalization, migration, codecs, schema
generation, and plan construction are not language features. They are ordinary
pure functions over immutable values.

Telora supplies functions, closures, recursion, pattern matching, modules, and a
small runtime data model. Domain policies such as merge, defaults, precedence,
and encoding live in libraries where they can be inspected, replaced, and
composed.

### A closed and bounded world

Module paths are statically known, dependencies are fixed, runtime `eval` is
absent, and genuine runtime input enters through explicit host values. Telora,
JSON, YAML, and TOML files participate in the same immutable module graph.

Telora permits recursion, but every execution has independent fuel, stack, call
depth, and allocation quotas. An execution deterministically produces a value
or a structured resource failure within its configured boundary. Failed work
is discarded atomically rather than partially published into the persistent
world.

### Diagnostics are first-class

Source locations travel with values through imports, transformations, metadata,
and codec normalization. A validation failure can identify both the data and
the rule that rejected it:

```text
user.yaml:4:8: expected Int
  User.telora:3:10: requirement declared here
```

JSON, YAML, and TOML files in the workspace are first-class source modules, not
opaque external blobs. They retain syntax diagnostics and field-level
provenance and participate in dependency and workspace analysis.

Incomplete Telora source still provides useful navigation, types, and
diagnostics. Semantic facts distinguish known values from explicit `Any`,
unknown information, conflicts, dependency blocking, and tool-stage
incomputability. Completion does not invent structure to appear helpful.

This feedback model is part of the language experiment, not an editor added
after execution works.

### Types are programmable metadata

A type declaration evaluates to canonical ordinary Telora data:

```telora
def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) {
    Option(Item)
};

type MaybeInt = Maybe(Int);
```

`Maybe` is an ordinary pure function evaluated by the same VM used for program
code. The type checker interprets its result rather than reimplementing the
function in a hidden type-level language.

The same metadata can drive static checking, LSP information, runtime
validation, normalization, codecs, documentation, schema generation, and
user-space interpreters. `TypeOf(A)` preserves the relationship between a
metadata witness and the values it describes; the narrow `Dyn` and
`interpreter!(...)` boundary supports heterogeneous interpretation without an
unchecked cast.

Types are central to Telora, but they serve the larger goal: programmable data
rules with authoritative, source-aware feedback.

## The Host Owns Effects

Telora has no authority over the external world. A host supplies explicit
ordinary inputs and decides whether an ordinary output has external meaning:

```text
external world
    -> host input snapshot
    -> closed Telora computation
    -> output value
    -> host validation and authorization
    -> external world
```

There is no universal Telora action ABI. A process launcher, build system,
Kubernetes controller, or agent runtime defines its own types and interprets
only the values it recognizes. Permissions, IO, retries, transactions, clocks,
and observation remain host concerns.

`telora run` uses a pure Edge Entry selected by the host. The Entry declares
its input needs, validates the Main export record, and reduces explicit system
events into effect descriptions. `check`, `query`, and LSP use fixed tooling
entries. Domain plans remain ordinary values interpreted by external hosts.

## What This Enables

### Codecs and schemas without language magic

Decorators are functions, attributes are data, and codecs are metadata
interpreters:

```telora
import "std/json" as json;

@json.rename_all('CamelCase)
@struct
type User = {
    user_id: Int,
    @json.default('None)
    nickname: Option(String),
};
```

Field renaming, defaults, flattening, and skip policies are library-defined
metadata. Encoding and decoding share one plan, and JSON Schema is generated
from that same plan.

Types can also declare textual parsing rules. `Regex` is a public standard
library native type; its expression is compiled during type construction, when
named captures are checked against the complete field set:

```telora
import "std/regex" as re;
import "std/string" as string;

@re.parse_by(re.compile(r"(?P<name>\w+)=(?P<value>\d+)"))
@struct
type Rec = { name: String, value: Int };
```

`string.parse(Rec, "answer=42")` has type `Result(Rec, BlameError)`. The type
is the authoritative contract; the regex only matches and splits a validated
textual representation. Captured fields are parsed recursively through the
same `std/string.parse` capability, so nested decorated struct types compose
without regex owning their conversions.

The reverse direction uses a separate `Display` capability for stable,
user-facing text:

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
@struct
type Endpoint = { host: String, port: Int };

fmt.display(Endpoint, { host: "localhost", port: 8080 })
```

The template is checked and compiled during type construction. Field
substitutions recursively use their types' Display capabilities, so nested
decorated structs compose without reparsing templates at runtime. Diagnostic
`Debug` output remains a separate future capability.

Types can explicitly make that text representation their structured-codec
container form:

```telora
@string.decode_by_parse
@string.encode_by_display
@fmt.display_by("{host}:{port}")
@re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
@struct
type Endpoint = { host: String, port: Int };
```

Within the semantic `Value` boundary, `codec.decode(Endpoint, value)` accepts a
`'String(...)` variant and `codec.encode(Value, endpoint)` produces one. JSON
Schema describes `Endpoint` as a string even when nested. The two bridge
declarations are paired and currently apply only to a type container; field
overrides are intentionally deferred.

### Deterministic plans and Edge entries

A module has no ambient authority. It exports ordinary values, including any
application-defined executable, build, query, or deployment plan. An external
host decides which plan type it accepts and how to interpret it.

`telora run app` selects `@bin/app.telora`. By default its built-in Entry emits
the explicit String `output` export. `--entry path/to/entry.telora` instead
authorizes a pure user Entry, whose `MainType` and output encoding are entirely
its own. The Entry runs outside MainWorld, may access the private/native modules
visible in the selected dependency graph, and exchanges only explicit
`SystemEvent` and `SystemEffect` values with the host. The initial effects cover
stdio child processes, process replacement, String output, and exit. Entry code
cannot perform IO itself: it reduces later child observations as events and
uses ordinary Telora codecs and formatters to produce output text.

### Static data as source

JSON, TOML, and YAML modules enter the same immutable graph as Telora code and
export exactly `{ data: Value }`. `std/value.Value` is one nominal recursive
tagged sum shared by static imports and `json/yaml/toml.parse`; it is semantic
data, not a lossless syntax tree. Typed models cross this boundary through
`codec.decode(Model, value)` and `codec.encode(Value, model)`.

TOML temporal categories retain distinct Value variants. YAML uses a fixed
conservative schema: legacy implicit booleans and timestamps remain Strings,
mapping keys must be Strings, custom tags are rejected, aliases are bounded,
and mapping merge keys are expanded deterministically. Standard `!!binary`
maps to Bytes after strict base64 validation.

### Conservative local polymorphism

An unannotated closure-valued `let` can infer a rank-1 scheme:

```telora
let identity = fn(value) { value };
(identity(1), identity("text")) # (Int, String)
```

Inference is intentionally bounded. Aliases instantiate once, recursive groups
remain monomorphic without an explicit contract, and numeric constraints are
not erased into unconstrained parameters. Telora prefers an explicit unknown or
diagnostic over unstable inferred precision.

## Agentic Systems

Machine-generated programs make Telora's constraints more valuable. Generation
is cheap; trustworthy feedback and controlled external meaning are not.

Telora can act as a typed, source-aware IR for plans. An agent generates or
modifies a pure program; Telora returns a complete plan value that a host can
validate, compare, review, sign, or reject before any effect occurs. The plan's
action vocabulary remains ordinary host-defined data.

Telora can also define one pure step of a host-driven loop:

```text
Context x State x Observation
    -> Result(LoopDecision(State, Plan, Output), BlameError)
```

The host owns observation, persistence, time, effects, retries, approvals, and
the overall loop budget. Telora computes one deterministic, finitely bounded
transition. Its diagnostics can point back to generated Telora, a JSON/YAML/TOML
source value, and the rule that rejected it, creating a precise repair and
audit loop.

These uses require no Agent-specific syntax and grant Telora no additional
authority.

## Design Tradeoffs

- **Compared with CUE:** Telora does not make unification the foundational
  semantics of constraints and composition. Policies are explicit functions
  over data.
- **Compared with Dhall:** both value pure, reproducible computation. Dhall
  guarantees normalization; Telora permits recursion and supplies deterministic
  fuel and resource boundaries.
- **Compared with Starlark:** both support controlled hosted computation. Telora
  additionally makes programmable type metadata, source provenance, partial
  semantic facts, and editor feedback part of the core experiment.
- **Compared with Nickel:** Nickel makes contracts, merging, and priorities
  central configuration mechanisms. Telora keeps such policies in replaceable
  libraries.
- **Compared with a sandboxed scripting language:** Telora is not only bounded.
  It unifies static data, transformation code, rules, runtime validation, and
  tooling in one source-aware semantic model.

Telora does not eliminate complexity. It tries to place domain complexity in
ordinary libraries and data while keeping the trusted language semantics small
and consistent.

## Current Boundaries

Telora is experimental. It has no language-level effects, ambient IO, dynamic
imports, general package acquisition, traits, or type narrowing. Hosts may
provide narrow adapters, but effects are not a deferred part of the language.

The project has now demonstrated the central vertical path, including computed
and recursive type metadata, derived codecs and schemas, recoverable workspace
semantics, a language server, bounded rank-1 inference, safe dynamic
observation, and user-space reference Equality and Show interpreters. It has
not yet demonstrated production-scale hosts, long-term compatibility, or broad
external use.

Likely application domains include reusable configuration packages, build and
toolchain planning, continuous reconciliation, policy-driven data pipelines,
typed Agent plans, and host-driven Agent loops.

## Try It

```sh
cargo run -p telora -- check examples/mvp/main.telora
cargo run -p telora -- run examples/mvp/external.telora --input examples/mvp/request.json
cargo run -p telora -- query at @src/compiler.telora -C examples/analytics-ontology
cargo run -p telora -- lsp
```

## Documentation

- [docs/README.md](docs/README.md): the current design SSOT, document map, and
  update rules (Chinese)
- [docs/MOTIVATION.md](docs/MOTIVATION.md): the MRT problem domain and why
  Telora is a language for lowering intent and harnessing agents (Chinese)
- [docs/design/LANGUAGE.md](docs/design/LANGUAGE.md): the current whole-language
  design baseline (Chinese)
- [docs/design/CONCEPT.md](docs/design/CONCEPT.md): the authoritative core
  concepts, ownership boundaries, and dependency direction (Chinese)
- [INTRO.md](INTRO.md): the problem domain, prior art, and the GCC wrapper case
- [VISION.md](VISION.md): the design thesis and feature admission rule
- [tutorial.md](tutorial.md): the current public language tutorial (Chinese)
- [rfc/](rfc/): decision history and implementation acceptance evidence
- [README.zh.md](README.zh.md): Chinese introduction

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
