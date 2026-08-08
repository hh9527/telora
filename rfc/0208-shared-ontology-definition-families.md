# RFC 0208: Shared ontology definition families

- Status: Implemented
- Depends on: RFC 0203, RFC 0207

## Summary

Add three executable TypeMetadata families and require both enterprise models
to instantiate them:

```text
MeasureDefinition(Id, Entity, SemanticType, Aggregation, Input, Output)
DimensionDefinition(Id, Input, Output)
RelationDefinition(Entity, Cardinality, Mapping)
```

The families define semantic roles, not concrete enterprise identities.

## Definitions

`MeasureDefinition` generates a record containing:

- model-specific measure identity;
- semantic value type;
- natural grain entity;
- aggregation classification; and
- `Fn(Id, Input) -> Option(Output)` lowering.

`DimensionDefinition` generates identity plus a model-specific typed lowerer.
The required entity remains in Output because some dimensions derive multiple
requirements or reject before a requirement exists.

`RelationDefinition` generates semantic endpoints and cardinality plus a
model-specific Mapping payload. The shared family therefore knows that a
relation connects Entity values under a cardinality rule, but does not know
table names, aliases, columns, or expression syntax.

## Acceptance criteria

1. B2B and B2C both instantiate all three families;
2. no family uses `Any`, Dyn, or String for a generic semantic parameter;
3. B2B retains SemanticValueType, Aggregation, Alignment, QueryPlan,
   GroupRequirement, Cardinality, and RelationMapping precision;
4. B2C retains its distinct corresponding types;
5. B2B and B2C relation mappings may have different structures;
6. physical join rendering remains model-owned; and
7. established valid and invalid behavior remains unchanged.

## Implementation result

`ontology-method/types.telora` now exports all three constructors. B2B replaces
its handwritten MeasureCapability and DimensionCapability records with
generated types. Its Relation becomes:

```telora
type Relation = types.RelationDefinition(
    Entity,
    Cardinality,
    RelationMapping,
);
```

where RelationMapping retains table, alias, and both join-column sides.

B2C uses the same families. Its relation mapping remains the smaller
`{table: String, on: String}` record, and its measure definition adds explicit
semantic value and aggregation enums. The difference demonstrates payload
polymorphism rather than a least-common-denominator mapping.

These constructors are now **constructed by both** enterprise models under the
RFC 0207 evidence taxonomy. `Maybe`, `Many`, Requirement, and the older generic
Capability constructor remain fixture-only or available-only and are not
counted as enterprise reuse.

## Remaining boundary

The constructors still return erased `Type` because a user family cannot name
its result as `TypeOf(MeasureDefinition(...))` inside another generic scheme.
Concrete declarations evaluate the metadata and recover the full represented
record, so model values are strictly checked. Shared functions must remain
generic over the instantiated record and use typed projections.
