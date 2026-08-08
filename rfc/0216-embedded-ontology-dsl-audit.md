# RFC 0216: Embedded ontology DSL audit

- Status: Implemented
- Depends on: RFC 0214, RFC 0215

## Summary

Audit the first analytics ontology DSL against the intended enterprise
boundary: an enterprise should state what only it knows, while an industry
method library owns repeated compilation knowledge.

## What was built

The implemented stack is:

```text
Telora language mechanisms
    TypeOf, TypeMetadata functions, higher-order functions, modules,
    Option publication, provenance-preserving diagnostics

ontology-method
    definition families, capability compilation, path classification

analytics-ontology
    typed measure/dimension/path/publication compiler protocol

B2B and B2C enterprise modules
    concrete ontology facts, policy, and physical plans
```

This is an embedded DSL in the substantive sense. `analytics-ontology` defines
the vocabulary of extension points and the semantics that compose them. An
enterprise model is a typed program in that DSL. No new syntax or compiler
branch distinguishes it from ordinary Telora code.

## Third-enterprise authoring contract

A third analytics enterprise currently needs to provide:

1. closed Entity, Measure, Dimension, and plan types;
2. measure and dimension capability arrays with concrete lowering functions;
3. safe and fan-out relationship facts with physical mapping payloads;
4. a measure-combination policy returning one typed combined measure plan;
5. typed adapters for capability ids/lowerers and required-node subjects;
6. any additional path requirements produced by filters or other stages; and
7. a final builder that lowers verified semantic inputs to its physical plan.

It does not need to rewrite capability search, per-request independent
lowering, partial-result collection, compilation completeness, required-node
combination, graph closure, path classification, dimension path diagnostics,
or the atomic publication gate.

## Shared industry knowledge

The shared compiler now maintains one order-sensitive invariant for both
enterprises:

```text
independently lower capabilities
-> combine available measure fragments
-> derive relationship requirements
-> classify safe/fan-out/missing paths
-> run downstream independent diagnostics
-> publish only with complete evidence
```

The “run diagnostics before publication gate” rule is important. The first B2B
migration exposed that an early gate hid an unrelated invalid render field.
The corrected shared rule evaluates a candidate builder for diagnostics, then
discards its value when capability or path evidence is incomplete.

## Enterprise knowledge that correctly remains local

Neither enterprise shares concrete ids, tables, expressions, joins, measures,
dimensions, grain policy, or output shape. B2B additionally owns restrictions,
filters, SQL AST lowering, rendering, and a Host wire plan. B2C owns its simpler
plan directly. Moving any of these into `analytics-ontology` would confuse one
enterprise implementation with industry methodology.

The measure combiner and final builder are deliberately callbacks. They encode
real business and publication policy, not adapter boilerplate.

## Boilerplate audit

Each model supplies eight small selector or forwarding closures:

- two capability-id selectors;
- two capability-lowering adapters;
- combined-measure base and required-node selectors; and
- dimension required-node and diagnostic-subject selectors.

B2B additionally maps filter requirements to nodes. These closures contain
little enterprise knowledge. They exist because a generic Telora function
cannot express a model with associated Entity/Measure/Dimension/Plan types or
constrain a user-generated metadata record and project its known fields.

The cost is visible: `compile_with` has a long positional interface. The B2C
model decreased from 252 to 237 lines, while the richer B2B model changed from
701 to 712 lines after the explicit imports and callbacks. Line count is not
the success metric; one shared implementation now owns the phase invariant.
Still, these numbers prevent claiming an ergonomic victory that has not yet
occurred.

## Language and method gaps

1. A user-generated type family still cannot be named precisely as
   `TypeOf(F(A))` inside another generic contract.
2. Telora has no associated model-family expression such as `Model.Entity` or
   structural constraint for the generated capability records.
3. Continuation-style construction and selectors preserve type safety but make
   APIs wider than their conceptual model.
4. Required nodes originating from measures participate in path planning, but
   the current reusable diagnostic rule has authored subjects only for
   dimensions. A missing measure dependency can therefore lack an equally
   precise domain diagnostic.
5. Relationship closure remains bounded to six expansion rounds.
6. The two enterprises share an analytics workflow; this does not prove that a
   toolchain, deployment, or another industry ontology should use it.
7. Code Agent readability and repair convergence have not yet been measured by
   a controlled generation loop.

None of these gaps justifies erasing model types to `Any`, introducing a trait
system immediately, or adding ontology behavior to the VM.

## Honest result

The ideal is partially achieved and clearly valuable. Enterprise modules can
focus on their private facts and policies, while a separately maintained
Telora library controls substantial industry-known lowering and verification
behavior. Both models execute that behavior, so this is more than a type or API
sketch.

The result is not yet a terse enterprise authoring experience. Typed adapter
closures and the flat protocol are the main exposed mechanism debt. They are
acceptable sample code for now because they preserve closed types, provenance,
and readable policy boundaries.

The next meaningful evidence should be a Code Agent exercise using the shared
DSL, or a genuinely independent third analytics model. Further abstraction
without such evidence risks optimizing the fixture rather than the method.
