# Ontology eDSL design contract

This document specifies the observable behavior of a reusable ontology eDSL.
It deliberately does not prescribe function names, module layout, graph
algorithm, internal state representation, or teaching structure.

The goal is to let structurally different enterprises define private measures,
dimensions, relationships, physical mappings, and plan builders while one
domain-neutral library owns capability compilation, path policy, diagnostics,
and atomic publication.

## Ownership boundary

```text
Reusable eDSL
  semantic-role TypeMetadata families
  capability lookup and independent lowering
  completeness evidence and requirement collection
  path selection and classification
  diagnostic construction and publication policy
  atomic publication orchestration

Enterprise model
  closed identities and domain values
  capability definitions and formulas
  relation catalogs and physical mappings
  combination and final plan-building policy
```

The eDSL must contain no enterprise entity, table, column, SQL fragment,
formula, status code, or String-based identity. Enterprise models must not
duplicate the shared compilation and classification rules.

## Semantic role families

The eDSL exposes executable TypeMetadata families for at least these roles:

- **MeasureDefinition**: identity, semantic value, natural-granularity entity,
  aggregation classification, and a model-specific lowering capability.
- **DimensionDefinition**: identity and model-specific lowering capability;
  requirements derived by a dimension belong in its output payload.
- **RelationDefinition**: semantic from/to endpoints, cardinality
  classification, and an enterprise-owned physical mapping payload.

Field meanings are library-owned; all concrete identity, input, output,
entity, classification, mapping, and plan types are model-supplied. The
families and any promised classification types must be exported and consumable
from another module.

## Capability compilation

Given requested identities and a typed catalog, the shared protocol must:

1. check authorization and locate each capability independently;
2. invoke lowering with the original requested identity;
3. retain an aligned `Array(Option(Output))` or equivalent per-request
   evidence;
4. collect every completed value without fabricating a replacement;
5. derive and de-duplicate relation requirements through typed selectors; and
6. continue independent work after one expected domain rejection.

Missing, unauthorized, mismatched, or unsuccessful capabilities must be
observable as domain rejection evidence with authored subjects. They must not
be indistinguishable from an accidental runtime error.

## Path inputs

Path classification receives:

- a safe relation catalog;
- a fan-out relation catalog;
- a base node;
- ordered target requirements;
- typed endpoint selectors; and
- any typed equality capability required by the chosen API.

Every relation value, including its enterprise physical mapping, must survive
selection unchanged. A semantic edge in both catalogs is an invalid catalog
fact and produces a sourced diagnostic.

## Safe path selection

For every target that has a pure-safe path of at most eight edges from the
base, select one path using this policy:

1. choose the path with the fewest edges;
2. when paths have equal length, compare their safe-catalog index sequences
   lexicographically and choose the lower sequence; and
3. preserve base-to-target edge order in the selected path.

Combine selected paths in target order. If an edge occurs in more than one
selected path, retain its first occurrence. Consequently:

- no targets produce no selected safe edges;
- a reachable branch unrelated to every target is excluded;
- a multi-hop target contributes every edge on its selected path; and
- alternative paths are not all passed to the plan builder.

These are observable requirements, not an instruction to use a particular
search algorithm.

## Fan-out and missing classification

Full reachability uses the union of safe and fan-out catalogs and permits the
two edge classes to alternate along a path.

Within the eight-edge bound, each ordered target is classified as exactly one
of:

- **safe**: a pure-safe path exists;
- **fan-out-only**: no pure-safe path exists, but a path in the union catalog
  exists; or
- **missing**: no path in the union catalog exists within the bound.

Classification preserves target order. Fan-out and missing diagnostics retain
the authored requirement subject rather than blaming only a generic node.

The result also exposes whether either bounded traversal had an unexpanded
frontier after eight edges. This `truncated` evidence is `True` exactly when
the configured bound may have hidden further reachability. A truncated result
cannot authorize publication. Fuel exhaustion is a runtime failure, not
truncation evidence.

## Builder transport

The final enterprise builder must receive both:

- the validated combined semantic value; and
- the selected safe relation values in deterministic path order.

Passing only target nodes, dropping selected edges, or reconstructing mappings
from String names violates the contract. The builder must be able to consume
the enterprise-owned physical mapping carried by each selected relation.

The eDSL may choose a direct product, a named semantic input record, or typed
callbacks, provided the exact types and relation values are preserved.

## Diagnostic and decision channels

Expected model outcomes are represented explicitly. The compile result must let
a caller distinguish at least:

- a published plan;
- rejection because capability evidence is incomplete;
- rejection because paths are fan-out-only, missing, or truncated; and
- rejection by the enterprise builder.

It also retains per-request completion evidence and structured `BlameError`
values, or an equally precise typed representation. The eDSL must not use a
fatal reported diagnostic as the only representation of expected rejection.

Diagnostics preserve three origins when applicable:

1. intent: the requested identity;
2. model fact: the capability or relation involved; and
3. shared rule: the eDSL check that rejected it.

Independent diagnostics run before the final publication decision. A premature
gate must not hide an unrelated failure.

## Atomic publication

A plan may be published only when all of the following hold:

- every requested capability produced a value;
- path classification is not truncated;
- every required target has an accepted safe path under the policy above;
- independent downstream diagnostics have run; and
- the enterprise builder returned a plan.

Otherwise the result is explicitly rejected and contains no partial plan.

## Enterprise extension points

An enterprise supplies:

1. closed identity, entity, requirement, output, and plan types;
2. concrete measure and dimension capability catalogs;
3. safe and fan-out relation facts with physical mapping payloads;
4. typed identity, lowering, endpoint, requirement, and subject selectors;
5. authorization, semantic combination, and final plan-building policies; and
6. any additional requirements produced by other semantic stages.

It does not reimplement capability traversal, completeness, requirement
collection, path selection, classification, diagnostic ordering, or the
publication gate.

## Implementation freedom and constraints

A2 chooses the public API, file layout, helper functions, TypeMetadata family
shapes, internal state, and algorithm. The implementation must remain pure,
deterministic, typed, and domain-neutral.

Do not use `Any`, `Dyn`, String identity, Host-native graph operations, hidden
mutable state, or copied repository implementations. A bounded ordinary Telora
implementation is required.

The delivery includes:

- the eDSL source;
- `EDSL_TUTORIAL.md` for enterprise authors;
- `AI3_CONTRACT.md` listing required model inputs and eDSL guarantees; and
- `STAGE2_NOTES.md` documenting API choices, tradeoffs, and known risks.
