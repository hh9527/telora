# RFC 0206: B2C ontology-method reuse

- Status: Implemented
- Depends on: RFC 0205

## Summary

Define a second enterprise model over twelve B2C tables without importing or
copying the B2B model. Reuse the published ontology-method types and functions
for capability construction, lookup, independent lowering, completion, and
relation planning.

## Model pressure

The B2C model introduces twelve closed entities:

```text
Consumer, Household, Address, Region,
Session, Cart, CartItem,
Order, OrderItem,
Product, Category, Campaign
```

It defines PurchaseRevenue at Order grain and SessionConversions at Session
grain. Its dimensions include purchase month, consumer region, acquisition
campaign, product category, and a deliberately undefined loyalty tier.

This differs materially from B2B:

- acquisition attribution follows Order -> Session -> Campaign;
- consumer geography follows Consumer -> Household -> Address -> Region;
- ProductCategory crosses Order -> OrderItem and therefore requires an
  explicit one-to-many policy; and
- LoyaltyTier exists in the intent type but has no capability definition.

## Reused method

The model instantiates generated capability types:

```telora
type MeasureCapability =
    types.Capability(Measure, Alignment, MeasurePlan);

type DimensionCapability =
    types.Capability(Dimension, Array(Measure), DimensionPlan);
```

It uses `ontology.lower_requested`, `completed`, `all_complete`, `close_six`,
`select_connecting_edges`, and `contains` without changing their semantic API.
The model supplies only typed identities, callbacks, relations, and physical
mappings.

The Capability TypeMetadata constructor was aligned with the already-corrected
lowering protocol so its field is `Fn(Id, Input) -> Option(Output)`. This lets a
lowerer construct output from the original requested value and retain intent
provenance. It is a consistency correction discovered before the B2C model,
not a B2C concept added to the shared layer.

## Acceptance criteria

1. the model contains exactly twelve distinct closed Entity variants;
2. it imports ontology-method but no intelligent-reporting/B2B module;
3. valid PurchaseRevenue with month, region, and campaign lowers to a complete
   read-only plan;
4. the plan includes both geography and attribution relation paths;
5. invalid ProductCategory reports that the path expands measure grain;
6. invalid LoyaltyTier reports a missing shared capability;
7. both independent errors are available in one recovery snapshot;
8. model and intent code contain no `Any` fallback; and
9. no compiler, VM, or Host special case is added.

## Implementation result

`examples/b2c-reporting` contains the twelve-table schema, a Chinese domain
description, the typed model, and valid/invalid intents. The valid plan records
`b2c-model-v1`, Order as its base entity, physical expression mappings, six
safe joins, and `read_only: True`.

The invalid intent requests ProductCategory and LoyaltyTier. Best-effort
evaluation reports both the model-owned fan-out rule and the shared missing
capability rule. Successful and failing intent modules remain close to the
business question and do not mention joins or physical tables.

## Comparative result

The experiment supports a reusable ontology-definition method, with a bounded
claim:

- TypeMetadata functions let each model generate its own strongly checked
  capability records;
- higher-order functions reuse capability and relation behavior across models
  without sharing their enums or physical schemas;
- original requested values must flow through shared lookup to preserve
  provenance; and
- concrete models still own plan types and final assembly because those encode
  real domain differences.

The shared layer is more substantial than generic `map`/`compose`: it owns the
meaning of capability resolution, independent lowering, completeness, bounded
reachability, safe connecting edges, and missing-capability diagnostics. It is
not a universal ontology runtime.

## Remaining gaps

- A user-defined metadata family cannot yet name its precise generated result
  inside another generic `TypeOf(F(A))` scheme. Concrete instantiation plus
  typed projections is safe but verbose.
- Workspace `show` projects exported generic schemes through `Any` in the
  module result view even though definition slots and strict calls retain their
  quantified contracts. Tool presentation should become more faithful.
- `close_six` is deliberately bounded. Larger or cyclic industry graphs need a
  different explicit policy rather than an implied general graph engine.
- B2B and B2C still duplicate final plan assembly and some path classification.
  The types differ enough that further extraction needs a third model or a
  concrete repeated invariant, not abstraction by aspiration.
- The experiment proves authored examples and deterministic regressions, not
  that a Code Agent can reliably repair a large corpus of generated intents.
