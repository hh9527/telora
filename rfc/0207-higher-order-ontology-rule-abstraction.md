# RFC 0207: Higher-order ontology rule abstraction

- Status: Accepted
- Depends on: RFC 0202

## Summary

Deepen the B2B/B2C ontology experiment from shared capability dispatch and
graph helpers into shared model-definition and higher-order rule protocols.

RFC 0202 proved a useful but limited common layer. Its strongest honest common
evidence is five functions used by both models:

```text
lower_requested, completed,
contains, close_six, select_connecting_edges
```

Only B2C currently instantiates the shared TypeMetadata Capability constructor.
Both models still define relation/path records, path classification,
fan-out/missing verification, and compiler assembly independently. This RFC
does not count APIs used only by a construction fixture as enterprise reuse.

## Goal

Test whether Telora can abstract rules into higher-order rules while retaining:

- closed model-specific Entity, Measure, Dimension, and plan types;
- readable concrete domain facts and physical mappings;
- static correspondence between definition input and lowering output;
- intent and model provenance through shared rules; and
- domain-specific freedom where semantics genuinely differ.

The desired result is not fewer lines by itself. It is one shared rule
definition maintaining an invariant for both models.

## Evidence classes

The final audit classifies every claimed shared capability:

```text
constructed by both
    both models instantiate the same TypeMetadata constructor

executed by both
    both models call the same higher-order rule in valid or invalid lowering

tested by both
    regressions exercise the shared invariant through both model APIs

available only
    exported by the library but used by at most one model

fixture only
    used by ontology-construction probes, not an enterprise model
```

Only the first three classes count as proved enterprise ontology
infrastructure.

## Child sequence

1. RFC 0208 adds shared MeasureDefinition, DimensionDefinition, and
   RelationDefinition TypeMetadata constructors. B2B and B2C must both replace
   their corresponding handwritten record types without erasing concrete type
   parameters;
2. RFC 0209 extracts complete path classification and requirement-oriented
   fan-out/missing verification. Both models retain physical relation catalogs
   and model-specific subjects, but one shared rule owns the invariant;
3. RFC 0210 introduces a typed capability-compilation protocol that returns
   raw independent results, completed outputs, and completeness together. Both
   compilers use it for measures and dimensions, and common required-node
   collection is extracted where it removes a repeated invariant;
4. RFC 0211 audits actual use, corrects earlier overclaims, compares concrete
   code before and after, and completes this umbrella.

Each child lands independently. A shared abstraction may be narrowed or
rejected when it damages provenance, readability, or model type precision.

## Acceptance criteria

1. both models instantiate the same shared MeasureDefinition,
   DimensionDefinition, and RelationDefinition metadata constructors;
2. generated definitions retain each model's concrete identity, grain,
   semantic type, aggregation, input, output, cardinality, and mapping types;
3. both models call one path-classification rule and one path-verification
   rule;
4. shared verification reports the concrete authored Dimension or equivalent
   subject, not only a generic Node;
5. both models use one capability-compilation result protocol for measures and
   dimensions;
6. B2B SQL, restriction provenance, four-error recovery, and wire plan remain
   unchanged;
7. B2C valid paths and two-error invalid recovery remain unchanged;
8. no abstraction replaces closed types with String, `Dict(Any)`, or unchecked
   Dyn projection;
9. no ontology-specific compiler, VM, or Host behavior is added; and
10. the final audit clearly marks library exports not actually shared by both
    enterprise models.

## Candidate definitions

The exact field shapes are validated in RFC 0208, but the intended families
are ordinary metadata functions:

```text
MeasureDefinition(
    Id, Entity, SemanticType, Aggregation, Input, Output
)

DimensionDefinition(
    Id, Input, Output
)

RelationDefinition(
    Entity, Cardinality, PhysicalMapping
)
```

Concrete models remain free to choose their enum variants and payload types.
The shared family defines the role of fields such as identity, natural grain,
aggregation, lowerer, endpoints, cardinality, and physical mapping.

## Non-goals

- a universal ontology object or registry;
- requiring B2B and B2C plan stages to have the same concrete type;
- sharing SQL payloads, table names, status codes, metric formulas, or business
  policy;
- nominal higher-kinded types solely to improve surface syntax;
- unbounded graph search;
- counting generic arrays or equality as ontology infrastructure; or
- measuring success only by deleted lines.

## Stopping rules

Stop an extraction when:

- either model must weaken a previously closed type;
- diagnostics lose the intent subject while gaining only a generic rule
  location;
- the callback/selector list becomes harder to understand than the invariant
  it centralizes;
- a shared definition contains a B2B or B2C concept;
- one model needs dummy values for fields meaningful only to the other; or
- the alleged common rule still has separate model-owned implementations.

The expected outcome may remain a medium-sized analytics method rather than a
general ontology framework. The purpose of this phase is to locate that
boundary precisely.
