# RFC 0212: Embedded ontology DSL

- Status: Implemented
- Depends on: RFC 0207

## Summary

Treat the reusable ontology work as an embedded DSL hosted by Telora. The DSL
is made from ordinary modules, TypeMetadata functions, higher-order functions,
closed enterprise types, and provenance-preserving diagnostics. It does not
introduce a second parser, quotation system, macro language, or ontology
special case in the VM.

This phase extracts an analytics compiler skeleton above `ontology-method`.
Enterprise models may keep typed adapters, but should only need to define the
knowledge that distinguishes that enterprise: its entities, capabilities,
physical mappings, policies, and final execution-plan construction.

## Layering

```text
ontology-method
    generic metadata families, capability compilation, graph rules

analytics-ontology
    measure/dimension/path lowering protocol

enterprise model
    closed vocabulary, facts, mappings, policy, physical-plan builder
```

The middle layer is the embedded ontology DSL's first industry method. It is
not a universal ontology object model. Another industry may build a different
method over the same Telora mechanisms.

## Core question

For a hypothetical third enterprise, can its authors express only facts they
uniquely know without reimplementing industry knowledge?

Enterprise knowledge includes:

- table and entity vocabulary;
- measure formulas and dimension mappings;
- relation facts and physical join payloads;
- grain-combination, authorization, and publication policy; and
- the final physical plan shape.

Industry knowledge includes:

- capability lookup and independent lowering;
- complete-versus-partial result accounting;
- required-node collection;
- relationship-path classification;
- fan-out and unreachable-target rejection; and
- the common order in which those proofs gate final plan construction.

## Proposed protocol

`analytics.compile_with` is a typed higher-order function. It accepts closed
enterprise types and callbacks for the points that contain enterprise
knowledge. Conceptually it performs:

```text
compile measures
-> compile dimensions
-> combine measure outputs
-> collect required entities
-> classify and verify paths
-> prove both compilations complete
-> call the enterprise final-plan builder
```

Independent stages continue after recoverable failures so Host best-effort
evaluation can report multiple diagnostics. Publication remains atomic:
`Option(Plan)` is produced only when all required evidence exists.

Callbacks are not automatically evidence of poor abstraction. A typed
selector that preserves a model-owned type can be acceptable adapter code. A
large collection of callbacks that merely forwards a shared structural shape
is evidence of a missing language mechanism and must be recorded as such.

## Child sequence

1. RFC 0213 implements the typed `analytics.compile_with` skeleton and a
   focused fixture;
2. RFC 0214 migrates the twelve-table B2C model, preserving its valid plan and
   multi-error invalid diagnostics;
3. RFC 0215 migrates the richer B2B model where honest, then audits enterprise
   knowledge, adapter boilerplate, and remaining compiler orchestration; and
4. RFC 0216 completes this umbrella with the measured result and language
   gaps.

Each child is independently committed. An extraction may be narrowed when it
weakens a closed type, provenance, diagnostic recovery, or readability.

## Acceptance criteria

1. B2B and B2C execute one shared analytics compilation protocol;
2. enterprise Measure, Dimension, Entity, relation, and plan types remain
   closed and statically checked;
3. missing capabilities, fan-out paths, and unreachable paths retain authored
   intent provenance;
4. valid output plans and existing invalid diagnostic sets remain stable;
5. no shared interface widens model data to `Any`, `Dyn`, or String ids;
6. enterprise modules no longer sequence capability completeness and path
   verification themselves where that order is common;
7. the final audit distinguishes domain callbacks from mechanical adapters;
8. a third-model author can identify a bounded list of required definitions
   without copying the shared orchestration; and
9. no ontology-specific compiler, VM, Host, trait, associated-type, or effect
   mechanism is introduced.

## Stopping rules

Stop or narrow the shared compiler if:

- precise model types must become erased;
- a diagnostic points only into the shared library instead of authored intent;
- the enterprise must reconstruct the shared phase ordering around the helper;
- callback forwarding dominates the enterprise-facing API;
- B2B-only SQL, rendering, or restriction concepts leak into the shared layer;
  or
- success depends on claiming generated sample facts as shared methodology.

## Non-goals

- a new ontology syntax;
- nominal higher-kinded types, traits, or associated types;
- eliminating all enterprise adapter code;
- sharing concrete B2B and B2C facts;
- unbounded graph search;
- a universal SQL compiler; or
- claiming that every domain ontology follows the analytics method.

## Implementation result

RFCs 0213 through 0216 implement and audit the first analytics ontology DSL.
Both enterprise models now execute one typed `compile_with` pipeline while
retaining their closed vocabularies, mappings, combination policy, and final
physical builders. Existing valid plans and best-effort diagnostic sets remain
covered by the workspace regressions.

The result validates the embedded-DSL architecture, not a claim of universal
ontology abstraction. RFC 0216 records the enterprise authoring contract and
the still-visible selector and positional-call costs.
