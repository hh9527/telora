# Published Telora interface

Import the public compiler and closed intent types with:

```telora
import "enterprise-model/lib.telora" { compile, Intent, MeasureId, DimensionId };
```

The exported declarations are:

```text
MeasureId = OrdersCreated | DeliveredPackages | UnitsShipped
DimensionId = OrderMonth | CustomerTier | OriginRegion | CarrierName | ServiceName |
              ProductCategory | ProductSku | DeliveryException
Intent = { measures: Array(MeasureId), dimensions: Array(DimensionId) }
compile: Fn(Intent) -> Option(ExecutionPlan)
```

`ExecutionPlan` is an opaque result. Intent authors construct only `Intent` values and call
`compile`; they do not construct or inspect plans.
