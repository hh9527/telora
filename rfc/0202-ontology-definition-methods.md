# RFC 0202: Reusable ontology-definition methods

- Status: Implemented
- Depends on: RFC 0051, RFC 0055, RFC 0192, RFC 0199

## Summary

Extract the definition method currently embedded in the intelligent-reporting
example into an ordinary Telora ontology library. The library should let two
different enterprises define typed measures, dimensions, relations, grains,
and physical mappings without copying the verification and lowering method.

The target separation is:

```text
ontology-method
    higher-order metadata constructors, capability protocols, and planners
        ↓
enterprise model
    Entity, Measure, Dimension, relations, semantics, and physical mappings
        ↓
enterprise intent
    one requested analysis
        ↓
verified execution plan or source-linked domain diagnostics
```

The experiment retains the existing ten-table B2B model and adds a distinct
twelve-table B2C model. Table count is only test pressure. Reuse is measured by
shared semantic construction and lowering, not by hiding table names behind a
common dictionary.

## Central hypothesis

Telora does not need a dedicated parameterized-type declaration syntax before
attempting this extraction. Types are metadata values, so ordinary functions
can construct types:

```telora
def Maybe:
    for(T) Fn(TypeOf(T)) -> TypeOf(Option(T)) =
    fn(inner) { Option(inner) };

type MaybeInt = Maybe(Int);
```

The ontology library should use this mechanism together with higher-order
functions. It may accept concrete model types and selector/lowering functions
as parameters. It must not erase model identities to `String`, flatten all
capabilities to `Dict(Any)`, or require compiler recognition of ontology
vocabulary.

This hypothesis is tested before adding a kind system, traits, associated
types, higher-kinded types, or ontology-specific VM operations.

## Shared concepts

The candidate ontology-method layer may define the construction and use of:

- entity and relation descriptions;
- relation cardinality and safe/fan-out reachability;
- measure identity, semantic value type, natural grain, and aggregation;
- dimension identity, required entities, projection, grouping, and result
  field semantics;
- capability lookup and independent lowering;
- grain alignment and completion policy;
- typed semantic and relational plan transitions; and
- provenance-preserving diagnostics for missing capabilities, incompatible
  grains, unreachable relations, and unauthorized requirements.

It may be generic over the concrete Entity, Measure, Dimension, relation,
expression, and plan types. SQL syntax, physical tables, business status
codes, and enterprise metric definitions do not belong in this layer.

## Concrete-model responsibilities

Each enterprise model owns:

- closed Entity, Measure, Dimension, and Filter types;
- the relation catalog and its physical join expressions;
- metric definitions such as revenue, units, refunds, or customer activity;
- the exact meaning of grain and aggregation for those metrics;
- mapping functions from semantic requirements to physical expressions;
- local restrictions and revision data; and
- any policy that legitimately differs between B2B and B2C.

The model should instantiate or consume ontology-method types and functions;
it should not reproduce graph traversal, completeness checks, capability
orchestration, or generic diagnostic structure.

## Child sequence

1. RFC 0203 verifies executable TypeMetadata constructors. It records which
   precise `TypeOf(T) -> TypeOf(F(T))` contracts work today, whether generated
   types cross modules, and where user-defined type-family precision widens to
   `Type`;
2. RFC 0204 creates the smallest ordinary `ontology-method` module that owns
   meaningful measure/dimension/relation protocols and their higher-order
   combinators. A standalone fixture proves its static and runtime contracts;
3. RFC 0205 separates the existing B2B example into the shared method, a
   concrete B2B model, and intents while preserving SQL, diagnostics, and wire
   plans;
4. RFC 0206 defines a structurally different twelve-table B2C model using the
   already-published ontology-method API. It adds valid and invalid intents and
   completes this umbrella with a comparative evaluation.

Each child is committed independently. RFC 0206 may add model data and
callbacks, but it may not silently redesign the shared API merely to make the
second example pass. A required shared change must be justified as a missing
method, tested against B2B, and recorded explicitly.

## Acceptance criteria

1. both enterprise models retain distinct closed Entity, Measure, Dimension,
   and Filter types;
2. both use one ordinary ontology-method module for semantically meaningful
   construction, verification, and lowering behavior;
3. the shared module contains no B2B/B2C entity names, table names, SQL
   fragments, status codes, or metric formulas;
4. concrete model code primarily states domain facts, mappings, and policies;
5. valid intents lower without exposing physical tables or SQL to the intent;
6. invalid intents retain concrete intent, model fact, and shared-rule source
   locations where those sources participate in the cause;
7. the existing B2B SQL results, four independent diagnostics, restriction
   behavior, and Host plan remain stable;
8. the B2C example exercises at least one relation shape, measure rule, or
   dimension policy not present in B2B;
9. no implementation uses `Any` or open strings to erase a type relationship
   that the concrete models currently check statically; and
10. the final report distinguishes proved reuse, model-specific complexity,
    erased fallback, and genuine Telora language gaps.

## Evaluation questions

- Can a model author define the desired concepts and mappings by composing
  readable Telora types and functions?
- Can a Code Agent understand the model's legal composition space without
  reading the shared implementation or generated SQL?
- Does adding the B2C model require new domain facts, or repeated changes to
  the supposed stable method?
- Are generated capability types visible and useful in annotations, hover,
  module interfaces, and diagnostics?
- When witness precision is unavailable, can the library preserve safety with
  a concrete generated type plus generic selectors, or does it require `Any`?
- Does successful lowering still produce one complete, executable plan whose
  remaining Host assumptions are explicit?

## Honest TypeMetadata boundary

Built-in constructors already expose precise families such as:

```text
Option : for(A) Fn(TypeOf(A)) -> TypeOf(Option(A))
Array  : for(A) Fn(TypeOf(A)) -> TypeOf(Array(A))
```

Ordinary `Fn(Type...) -> Type` functions can also generate concrete structural
types used by later declarations. The first child must test, rather than
assume, whether a user-defined family can name its generated result precisely
inside a generic scheme. If it cannot, this phase first tries concrete type
instantiation plus higher-order generic operations. It does not immediately
add higher-kinded types.

## Non-goals

- RDF, OWL, open-world inference, or a universal knowledge graph;
- one model type that contains every possible enterprise concept;
- sharing business definitions merely because both systems use SQL;
- nominal identity for every generated structural type;
- dynamic loading of arbitrary policy code;
- replacing enterprise differences with configuration strings;
- proving that two commerce examples cover every analytics domain; or
- changing database, package, or Host execution effects.

## Stopping rules

Stop and return to discussion when:

- the shared method needs a concrete enterprise entity or metric name;
- adding B2C repeatedly changes APIs already sufficient for B2B;
- generated types cannot cross module boundaries without `Any`;
- the method code is larger or harder to understand than both concrete
  implementations while removing no invariant maintenance;
- diagnostics point only into generic machinery and lose concrete model or
  intent causes; or
- a compiler or VM special case is required solely to recognize ontology
  declarations.

An honest outcome may be a small metadata-construction library plus an
analytics planner and two independent models. That is preferable to claiming a
reusable ontology language that exists only through erased data.

## Comparative implementation result

All four children are implemented. The ten-table B2B model and twelve-table
B2C model use one ordinary `ontology-method` package while retaining distinct
closed Entity, Measure, Dimension, Filter, capability, relation, and plan
types. Neither imports the other, and the shared package contains no business
identity, table name, physical expression, or metric formula.

The reused surface is materially domain-oriented:

- TypeMetadata functions generate concrete capability and requirement record
  types from each model's own types;
- higher-order capability lookup connects requested identities to typed
  lowerers and reports missing definitions;
- independent lowering and completeness prevent partial plans from being
  published while retaining best-effort diagnostics;
- generic relation closure and connecting-edge selection operate on each
  model's closed Entity and Relation types; and
- the original requested identity flows into lowerers so output provenance
  remains attached to the intent rather than the capability catalog.

B2B preserves its established SQL, four independent invalid diagnostics,
restriction provenance, and Host wire plan. B2C produces a typed read-only plan
through geography and acquisition-attribution paths, while one invalid intent
reports both a model-owned fan-out violation and a shared missing-capability
error.

## What did not become shared

Metric formulas, grain policies, physical mappings, concrete relation catalogs,
filter semantics, intermediate plan shapes, and final plan assembly remain
model-owned. This is intentional: they carry business meaning rather than
mechanical repetition. A future extraction needs another concrete repeated
invariant, not merely similar field names.

The shared API changed once during B2B integration. `lower_requested` initially
passed only Capability and Input; this caused output requirements to inherit
the model catalog's `id` provenance. Passing the original requested Id restored
intent provenance. The generated Capability constructor was aligned to the
same `Fn(Id, Input) -> Option(Output)` protocol before B2C used it. B2C required
no further semantic API change.

## Honest remaining boundaries

- Executable user metadata constructors work, but their generated family
  cannot yet be named precisely inside another generic `TypeOf(F(A))` scheme.
  Concrete instantiation plus typed projections is safe and somewhat verbose.
- Workspace module-result presentation widens quantified exports to `Any` even
  while strict checking and definition slots retain the generic contract.
- Relation closure remains an explicit six-step bounded policy.
- The examples prove reusable authored libraries and diagnostics, not yet Code
  Agent repair rates over a generated intent corpus.
- Two analytics models establish a credible industry method, not a universal
  ontology framework or applicability to every domain.

The central hypothesis is therefore supported within a clear boundary: Telora's
“types are metadata” model and ordinary higher-order functions can define a
reusable, strongly checked ontology construction and lowering method without a
trait system, higher-kinded types, or ontology-specific runtime machinery.

## Amendment after RFC 0207

The comparison above overstated one fact at the time RFC 0202 completed: only
B2C instantiated the shared Capability TypeMetadata constructor; B2B still
declared its capability records by hand. Shared generic functions were real,
but shared definition construction had not yet been proved by both enterprise
models.

RFC 0208 subsequently makes the stronger statement true by migrating both
models to shared MeasureDefinition, DimensionDefinition, RelationDefinition,
and Compilation families. That later evidence must not be read retroactively
as part of RFC 0202's original implementation result. RFC 0211 provides the
current evidence audit.
