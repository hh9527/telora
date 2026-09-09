# Telora Language Vision

## The Question

Telora is an experimental language built around one question:

> In a closed and pure world, what is the smallest language that can support
> programmable data transformation and validation, deterministic execution
> with a finite termination boundary, and first-class diagnostics and
> feedback?

Its name expands recursively as **TELORA Enables Lowering Objectives to
Reliable Artifacts**. Telora occupies the verified boundary between an agent's
objective and the artifacts a host may authorize in the real world.

The question is not primarily about type theory. Telora uses ideas from
programming-language research where they help answer it, but those ideas are
tools rather than the objective. The objective is a small, coherent data
programming environment that remains understandable when transformation logic
becomes real software.

Static configuration formats are easy to inspect but cannot express every
useful policy. General-purpose scripting languages are programmable, but their
open worlds, effects, ambient inputs, and weak data provenance make them hard
to reproduce, embed, audit, and diagnose. Sandboxing a scripting language with
fuel and a host API allowlist limits damage, but does not explain where a bad
value or rule came from, preserve useful facts through incomplete programs, or
give tools an authoritative semantic model.

Telora explores the space between those choices.

## The Four Requirements

### Programmable transformation and validation

Validation, normalization, migration, decoding, encoding, schema generation,
and plan construction are all transformations of ordinary data. Telora should
provide enough general computation to express them with functions, immutable
values, recursion, pattern matching, and modules.

The language does not define configuration-specific merge, priority, default,
or constraint semantics. Those are policies, and policies belong in ordinary
libraries that users can inspect, replace, and compose.

Validation is not a hidden language operation. It can be expressed as a
transformation with an explicit result:

```text
A -> Result(B, Error)
```

`Error` is a domain-defined error type, and `B` may be the original value or a
normalized domain value. A caller emits a diagnostic with `fail!(message, subject)`.
The same principle
applies to codecs and derived schemas: the language supplies computation and
data; libraries supply domain meaning.

### A closed and pure world

A Telora execution operates on an enumerable world:

- module paths are statically known;
- dependencies are fixed before execution;
- Telora, JSON, YAML, and TOML modules participate in the same immutable graph;
- runtime `eval` and arbitrary dynamic imports are outside the model; and
- genuine runtime inputs enter only through explicit host-provided values.

Ordinary values are immutable and functions are pure. The implementation may
use unobservable mutation for allocation, interning, caches, and publication,
but failed work must not leak partially initialized state into the persistent
world.

Closed does not mean that every value is statically known. It means that the
code, static data, dependency graph, and explicit inputs that may influence one
execution are bounded and identifiable.

### Deterministic, finitely bounded execution

Telora permits ordinary recursion. It does not require every program to be
strongly normalizing. Instead, every hosted execution has explicit limits for
evaluation fuel, stack use, call depth, and allocation. Within those limits an
execution deterministically produces a value or a structured failure.

Fuel is charged for operations that can expand the dynamic path, such as calls
and taken control-flow back edges. It is a termination boundary, not a virtual
CPU tariff. Harmless compiler lowering changes should not alter whether
straight-line code fits within a budget.

Determinism also includes representation and observation:

- collection shapes and output ordering are canonical where order has no
  domain meaning;
- module identities do not depend on incidental physical paths;
- lexical path operations do not observe the host filesystem;
- diagnostic and inference results do not depend on traversal order; and
- failed or stale analysis cannot publish partial state.

### Diagnostics and feedback are first-class

A data program is useful only if a human or an agent can understand what it
knows, why it failed, and where to make a repair. Diagnostics are therefore a
design input, not presentation added after the evaluator works.

Source origins travel through static data, values, transformations, type
metadata, and runtime failures. A validation error should identify both sides
of the relationship:

```text
config.yaml:12:16: expected Int
  model.telora:8:15: requirement declared here
```

JSON, YAML, and TOML files in the module graph are not opaque external blobs.
They are first-class source modules with syntax diagnostics, stable locations,
value provenance, dependency identity, and participation in workspace
analysis. Truly external values enter separately through a host window.

Incomplete source is normal during editing and generation. Recoverable syntax,
HIR, semantic facts, workspace revisions, and the language server should retain
independent knowledge around damage. The tooling distinguishes checked type
contracts from facts that are unknown, conflicting, blocked by a dependency,
or incomputable within the tool-stage budget. Incomplete facts retain their
status until enough evidence is available.

The same authoritative semantics should drive strict checking, runtime
validation, command-line inspection, and editor feedback.

## Types as Programmable Metadata

Types are a means to make transformation and feedback programmable without
splitting the system into separate schema, validation, codec, documentation,
and editor models.

A type declaration establishes a static type. Its explicit `.type` projection
produces canonical immutable metadata that can be passed to functions,
transformed, interpreted, and retained at runtime:

```telora
type Maybe(A) = enum { None, Some(A) };
type MaybeInt = Maybe(Int);
export def metadata: TypeOf(MaybeInt) = MaybeInt.type;
```

`Maybe` is a declared type family, not an ordinary function returning metadata.
Ordinary functions can consume and compose metadata, but their results cannot
be turned back into static types. Type declarations and trusted constructors
establish the skeleton; property providers and interpreters compute over it
using the toolchain-hosted Telora VM.

The same metadata may support:

- static checking and LSP information;
- runtime validation and normalization;
- encoding and decoding;
- documentation and schema generation; and
- user-space interpreters over heterogeneous typed data.

`TypeOf(A)` retains the relationship between a metadata witness and the values
it describes. `Dyn` permits safe existential packaging and structural
observation without becoming an unchecked cast. `interpreter!(...)` provides a
narrow, one-way bridge from statically witnessed values to user-space metadata
interpreters. It must not allow an interpreter to manufacture or recover an
arbitrary `A`.

Dedicated static machinery is justified only when ordinary data and tool-stage
functions cannot provide the required abstraction, safety, diagnostics, or
analysis. Greater type-system generality is not an objective by itself.

## One Evaluator Across Two Stages

Telora has a tool stage and a program stage, but both use the same value model,
function behavior, bytecode VM, quotas, and evaluation semantics.

- The tool stage evaluates closed metadata computations for checking, editor
  information, and derivation.
- The program stage evaluates ordinary data transformations.
- Type annotations are erased unless their metadata is explicitly used as a
  runtime value.

There is no separate macro language or unrestricted type-level language.
Elaboration may reduce surface conveniences to a smaller core, but it must not
create a second set of domain or stage-specific evaluation rules.

## A Small Runtime Data Model

The dynamic runtime is inspired by Lua's compact VM shape and Erlang's
immutable term model. Its basic value categories are:

```text
Int, Float, String, Bytes, Dict, Array, Atom, Tuple, Func
```

`Dict` is the sole native string-keyed product representation. Static Structs
and homogeneous `Dict<T>` values share that runtime form while retaining
different metadata. Atoms and tagged tuples express symbolic and sum values:

```text
None
Some(value)
Ok(value)
Err(error)
```

Boolean conditions accept only `True` and `False`; there is no general
truthiness coercion. Runtime representation stays small and uniform while
metadata and ordinary libraries provide richer interpretations.

## The Host Boundary

Telora has no authority over the external world. It does not need a general
effect system or a universal action ABI. A host opens a small, explicit window
by supplying ordinary input values and selecting a named export whose value
has external meaning:

```text
external world
    -> host freezes explicit input
    -> closed Telora computation
    -> explicit named module exports
    -> host selects a protocol entry
    -> host validates, authorizes, and interprets it
    -> external world
```

The VM does not know that a value describes a process, file, deployment, or
approval. Different hosts define different input and output protocols using
ordinary Telora types. Possessing a value of a plan type does not itself grant
the capability to perform that plan.

Modules have no default result. This includes `@main`: its only special status
is that it is selected by the host and cannot be imported. Host modes choose
their own named protocol entry, such as `output`, `exec`, or `build`.

The standard `telora run`, `telora exec`, and `telora build` commands are concrete
host adapters, not the beginning of a language-level effect system. Domain
semantics such as execution ordering, retries, transactions, permissions, and
real-world observation remain permanently owned by the host.

## Agentic Systems

Agentic software increases the value of Telora's constraints. Generated code is
cheap; trustworthy feedback and controlled external meaning are not.

Telora can serve as a source-aware, typed IR for plans. An agent may generate or
modify a pure Telora program, while the host receives a complete value that can
be checked, compared, reviewed, signed, or rejected before any effect occurs.
The plan vocabulary remains host-defined ordinary data.

Telora can also define the pure transition of a host-driven loop:

```text
Context x State x Observation
    -> Result(LoopDecision(State, Plan, Output), Error)
```

The host owns time, persistence, observation, effects, retries, approvals, and
the total loop budget. Telora computes one deterministic, finitely bounded step.
Diagnostics and provenance make the loop repairable and auditable: failures can
point to generated Telora, a JSON/YAML/TOML source value, and the rule that
rejected it rather than collapsing into an unstructured tool error.

These are opportunities enabled by the core model, not Agent-specific language
features.

## Admission Rule

Every proposed feature should answer three questions:

1. Is it necessary to express programmable data transformation or validation,
   or to provide authoritative diagnostics and feedback for that programming?
2. Can it instead be ordinary Telora code, metadata interpreted by a library,
   or behavior owned by a host?
3. Does it preserve the closed world, purity, deterministic observation, and
   finite execution boundary?

The language should grow only when the required capability cannot be expressed
or explained faithfully through existing mechanisms. A feature is not
justified merely because it is common in general-purpose languages or elegant
in a more general type system.

## Evaluation Criteria

The experiment succeeds when:

1. non-trivial transformation and validation policies can be ordinary Telora
   functions rather than language-specific rules;
2. static data and Telora source share one closed, source-aware module graph;
3. type metadata can be computed and interpreted without a hidden second
   evaluator;
4. strict checking, runtime validation, CLI queries, and LSP feedback agree on
   authoritative semantic facts;
5. every execution ends with a value or a source-aware resource failure within
   its configured bounds;
6. incomplete programs retain precise independent facts without fabricated
   certainty;
7. hosts can define useful input and output windows without adding domain
   semantics or effects to the language; and
8. new application domains primarily add Telora libraries and host adapters,
   not VM instructions or language constructs.

The project needs redesign if rich policies repeatedly require compiler
special cases, if metadata functions must be duplicated in a hidden type
language, if diagnostics lose the origin of transformed data or rules, or if a
new host domain requires Telora itself to acquire external authority.
