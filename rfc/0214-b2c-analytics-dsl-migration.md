# RFC 0214: B2C analytics DSL migration

- Status: Implemented
- Depends on: RFC 0213

## Summary

Migrate the twelve-entity B2C reporting model to
`analytics-ontology/compiler.telora` without weakening its closed model types
or changing its valid and invalid behavior.

## Enterprise-owned code

The model still defines all knowledge unique to this enterprise:

- Entity, Measure, Dimension, semantic type, aggregation, and cardinality;
- metric formulas and dimension expressions;
- the relation catalog and physical join mappings;
- the policy that only same-grain measures combine; and
- the concrete `ExecutionPlan` builder.

`CombinedMeasure` is a typed adapter carrying the result of the enterprise
grain policy into the shared protocol. It is not shared ontology knowledge.

## Removed orchestration

The model no longer directly:

- calls `compile_requested` twice;
- computes both completeness gates;
- combines measure and dimension required nodes;
- invokes path classification and verification; or
- sequences those checks before final publication.

Those invariants are now maintained by `analytics.compile_with`.

## Adapter audit

The call provides eight small selector/forwarding functions: two capability id
selectors, two capability lower adapters, a base selector, a measure-node
selector, and dimension node/subject selectors. The four relation and builder
arguments are model facts or policy. The selectors preserve precise generated
record types but expose the current lack of structural constraints or
associated model types.

## Implementation result

The valid fixture still produces the same `b2c-model-v1` plan with Region and
Campaign joins. The invalid fixture still independently diagnoses the missing
LoyaltyTier capability and ProductCategory fan-out. No value is erased to
`Any`, `Dyn`, or String identity.
