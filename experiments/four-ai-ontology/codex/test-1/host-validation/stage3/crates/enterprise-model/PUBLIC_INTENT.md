# Logistics fulfillment analytics intent

The public entry point is `compile(Intent) -> Option(ExecutionPlan)`. An `Intent` contains a
non-empty list of measures and a list of dimensions. A successful result is a typed, read-only
plan; failure returns no publishable plan and reports the applicable policy diagnostics.

## Measures

- `OrdersCreated`: number of orders created, at order grain.
- `DeliveredPackages`: number of delivered packages, at package grain.
- `UnitsShipped`: number of shipped product units, at package-item grain.

## Dimensions

- `OrderMonth`: month in which an order was created.
- `CustomerTier`: customer service tier.
- `OriginRegion`: shipping origin region.
- `CarrierName`: carrier used for fulfillment.
- `ServiceName`: selected service level.
- `ProductCategory`: product category.
- `ProductSku`: product SKU.
- `DeliveryException`: delivery exception classification.

All names above are members of closed vocabularies. `DeliveryException` is intentionally not an
approved capability. Product dimensions are approved capabilities, but they cannot be used from
order grain without an explicit pre-aggregation or allocation policy. That policy is currently
unavailable. Measures at different natural grains likewise cannot be combined without an explicit
policy, which is currently unavailable.

Compilation can fail for an empty measure selection, an unavailable capability, incompatible
measure grains, a grain-expanding relationship, a missing validated relationship, or failure to
construct a complete candidate. Independent errors are reported where possible, and an incomplete
or invalid candidate is never published.
