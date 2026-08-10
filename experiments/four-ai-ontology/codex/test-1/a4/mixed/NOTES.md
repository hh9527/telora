# Intent Notes

The request maps exactly to the public `OrdersCreated` and `DeliveredPackages` measures and the
`OriginRegion` dimension. The measures have different natural grains, and the published contract
states that the policy required to combine them is unavailable. The public compiler should
therefore reject this representable intent and report the incompatible-grain diagnostic rather
than publish a plan.
