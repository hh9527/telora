# Stage 4 Intent Trials

All six trials used separate Git workspaces and the frozen public intent tutorial, public enterprise
vocabulary, declaration-only API, role contract, and one request. Expected Host classifications
were not staged for A4. A4 did not execute Telora; Host validation used the frozen Stage 2 and
Stage 3 artifacts in separate validation directories.

| Trial | Host result | Classification | Correction rounds |
|---|---|---|---:|
| direct | Order plan; Region and Carrier safe mappings | `lowered` | 0 |
| novel | PackageItem plan; Category and Region safe mappings | `lowered` | 0 |
| unapproved | unavailable capability diagnostic; no plan | `model-rejected` | 0 |
| mixed | incompatible natural-grain diagnostic; no plan | `model-rejected` | 0 |
| fanout | grain-expansion relationship diagnostic; no plan | `model-rejected` | 1 |
| impossible | explicit closed-vocabulary refusal; no executable intent | `agent-refused` | 0 |

## Host Evidence

- `direct`: check succeeded with 7 dependencies. Run returned `Some` with base `Order`,
  `OrdersCreated`, dimensions `OriginRegion` and `CarrierName`, `read_only: True`, and complete
  Order-to-Region and Order-to-Carrier safe mappings.
- `novel`: check succeeded with 7 dependencies. Run returned `Some` with base `PackageItem`,
  `UnitsShipped`, dimensions `ProductCategory` and `OriginRegion`, `read_only: True`, and complete
  Category and Region safe mappings.
- `unapproved`: check succeeded with 7 dependencies. Run exited 1 at `DeliveryException` with
  `no capability is defined for the requested id`; no plan was published.
- `mixed`: check succeeded with 7 dependencies. Run exited 1 with
  `measures at different natural grains require an explicit pre-aggregation policy`; no plan was
  published.
- `fanout`: check succeeded with 7 dependencies. Run exited 1 with
  `relationship expands the measure grain; define explicit pre-aggregation or allocation policy`;
  no plan was published.
- `impossible`: A4 explicitly identified that the closed `MeasureId` lacks average delivery
  duration and `DimensionId` lacks weather condition. It emitted no executable `Intent`, invented
  identifier, SQL, physical mapping, or hand-built plan.

## Fanout Correction

The first `fanout` delivery refused too early even though both `OrdersCreated` and
`ProductCategory` are public vocabulary members. Main provided one bounded semantic correction
using only the public contract: construct the public intent and let enterprise `compile` decide the
missing grain-expansion policy. A4 then produced the representable intent, and Host observed the
expected model diagnostic. No hidden classification or enterprise implementation was disclosed.

## Totals

- Lowered: 2/6
- Model-rejected: 3/6
- Agent-refused: 1/6
- Incorrect acceptance: 0/6
- False rejection: 0/6
- Stage 4 bounded semantic correction rounds: 1
- SQL, physical-plan, and hand-built-plan bypasses: 0
