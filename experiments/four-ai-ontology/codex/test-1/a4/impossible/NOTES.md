# Refusal

The request cannot be represented faithfully by the published intent interface.

- `MeasureId` has no measure for average delivery duration in hours.
- `DimensionId` has no dimension for weather condition.

Because both vocabularies are closed, substituting another measure or dimension would change the
requested semantics. No compilable `Intent` is emitted.
