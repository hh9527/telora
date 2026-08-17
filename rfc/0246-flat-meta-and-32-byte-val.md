# RFC 0246: Flat Meta and 32-Byte Val

- Status: Proposed
- Tracking issue: #88
- Depends on: RFC 0245

## Summary

Telora will replace `RichValue { RuntimeValue, Provenance }` with a stable
32-byte `Val`. `Meta` is one packed `u32` with an exact kind, an exact Heap
sub-kind, representation traits, and provenance. All values are constructed
through checked internal constructors; arbitrary Meta combinations are not a
valid internal API.

## Meta layout

```text
bits  0..6   exact kind       (64 codes)
bits  6..12  Heap sub-kind    (64 codes; zero for non-Heap values)
bits 12..28  representation traits
bits 28..30  provenance       (Unknown, Original, Generated)
bits 30..32  reserved
```

The initial traits are representation facts used on hot paths:

```text
REFERENCE  raw names an arena slot or handle
LOCAL      the reference belongs to the current WorkWorld
TEXT       the reference names interned String/Atom text
INLINE     raw contains the complete scalar payload
HEAP       raw names a general Heap object
UPLINK     raw names an uplink cell
TRACE      the referenced payload may contain Val edges
```

Exact kind and sub-kind remain authoritative. Traits are redundant query bits,
not independently mutable facts. Compile-time Meta constants and private
constructors produce the only valid combinations. Debug assertions validate
kind/sub-kind/trait consistency at Heap access and copy boundaries.

## Packed location

`PackedLoc` stores source, byte start, and byte end as three `u32` words.
Unknown provenance permits a zero source word. Original versus generated
origin is stored in Meta so `Loc` does not sacrifice source-ID space or invent
invalid `NonZeroU32` values.

## Equality

Source location and provenance do not participate in value equality. Physical
equality may compare Meta classification, witness, and raw bits only where the
kind defines raw identity. String/Atom references retain content equality, and
Heap composites retain their existing logical equality traversal.

## Migration

The first implementation may retain a private compatibility facade exposing
the old constructor names while their storage is changed to `Val`. It may not
retain the Rust enum as a second runtime representation. Heap objects and
legacy Host `Value` are migrated by explicit conversions.

## Acceptance criteria

1. compile-time assertions fix `Val` size to 32 and alignment to 8;
2. all exact kinds and Heap sub-kinds round-trip through Meta;
3. trait masks agree with every valid Meta constant;
4. Unknown, Original, and Generated provenance retain existing rebase behavior;
5. VM registers and Heap child arrays store `Val`; and
6. the complete workspace test suite passes before RFC 0247 begins.

