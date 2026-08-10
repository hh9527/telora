# Intent Notes

The requested product-unit count maps directly to the public `UnitsShipped` measure. The requested
grouping uses the public `ProductCategory` and `OriginRegion` dimensions. Because `UnitsShipped`
has package-item grain, using a product dimension does not require the unavailable policy described
for order-grain measures.
