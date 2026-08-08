# RFC 0213: Typed analytics compiler skeleton

- Status: Implemented
- Depends on: RFC 0212

## Summary

Add an `analytics-ontology` package whose `compile_with` function owns the
common measure, dimension, relationship, and publication sequence while every
model type remains a caller-selected type parameter.

## Contract

The caller supplies:

- requested measure and dimension ids;
- typed capability catalogs, identity selectors, lowerers, and inputs;
- a measure-combination policy;
- required-node and diagnostic-subject selectors;
- safe and fan-out relation catalogs plus endpoint selectors; and
- a final typed plan builder.

The shared function performs both capability compilations independently,
collects required nodes, classifies paths, reports invalid dimension paths,
checks compilation completeness, and invokes the final builder only when all
evidence succeeds.

## Why callbacks remain

Telora cannot currently quantify over a generated structural family and then
project its fields as an associated type. Explicit selectors preserve closed
enterprise types without `Any` or `Dyn`. Measure combination and final plan
construction are genuine enterprise policy; the final audit separately counts
selectors that are only mechanical adapters.

## Diagnostics

The function does not short-circuit the dimension compilation after a measure
failure. It operates on completed partial values for downstream independent
checks, while `Option(Plan)` remains gated by both complete compilations,
measure combination, path validity, and the enterprise builder.

## Implementation result

Implemented in `examples/analytics-ontology/src/compiler.telora`. The package
depends only on `ontology-method`; the implementation is ordinary Telora and
adds no compiler or Host behavior.
