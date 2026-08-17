# RFC 0245: Fixed-Width Flat Runtime Values

- Status: Proposed
- Tracking issue: #88
- Depends on: RFC 0012, RFC 0021, RFC 0023, RFC 0103, RFC 0237, RFC 0241

## Summary

Telora will represent every internal runtime value as one fixed 32-byte `Val`.
The value carries source provenance, a flat physical representation tag, a
canonical declared-type witness, and either immediate bits or a World-relative
reference. Narrowing a validated value records its witness without allocating
an owned wrapper. Composite values remain exclusively in VM Heaps.

This is an umbrella RFC. RFC 0246 defines the `Val` and `Meta` layout, RFC 0247
defines inline text and World references, RFC 0248 defines canonical witnesses
and allocation-free narrowing, and RFC 0249 migrates graph copying and legacy
boundaries and records final evidence.

## Representation

```rust
#[repr(C, align(8))]
struct Val {
    loc: PackedLoc, // three u32 words
    meta: Meta,     // one u32 word
    ty: u32,        // a Main/Local type-arena slot selected by Meta
    narrow: u32,    // reserved for trait/interface narrowing evidence
    raw: u64,       // immediate bits or a reference payload
}
```

`size_of::<Val>()` and `align_of::<Val>()` are respectively 32 and 8 on every
supported target. `Val` is copied by value. Rust ownership containers do not
represent VM graph identity.

## Semantic layers

The fields describe separate facts:

- `kind` says how to decode `raw`;
- `sub-kind` identifies the physical Heap object without describing its
  language type;
- trait bits support branch-free or single-mask questions about representation;
- `ty` is a scoped `u32` language-level type witness;
  and
- `narrow` is an independent slot reserved for future trait/interface
  refinement evidence;
- `loc` and provenance describe source origin without affecting equality.

`Bool` is not a runtime kind. `'True` and `'False` are inline Atoms and may
carry the `Bool` witness. `Fail` is not a runtime kind either: it is the failing
internal form of `Never`.

## Flat kinds

The initial exact kinds are:

```text
Never
Int
Float
IString
String
IAtom
Atom
NativeType
Heap
Uplink
```

Heap sub-kinds initially cover `Bytes`, `Array`, `Tuple`, `Tagged`, `Dict`,
`Func`, `Type`, `Opaque`, and `Dyn`. The public `ValueKind` remains a logical
API projection and is not this physical taxonomy.

## World invariants

1. Main references remain valid in every WorkWorld attached to that MainWorld.
2. Local references are meaningful only with their owning WorkWorld.
3. A Local reference may cross a World boundary only through relocation.
4. Relocation installs a forwarding entry before traversing child edges.
5. Main references encountered during relocation are retained unchanged.
6. String and Atom references relocate by content; general Heap references
   relocate by graph identity.
7. Uplinks remain private recursive-graph edges and cannot cross a public Host
   boundary unresolved.

## Narrowing invariants

Reference-bearing fields use a `ScopedId(u32)`: bit 31 is zero for Main and one
for Work, while bits 0 through 30 hold the arena slot. `ty == 0` means no
witness; valid witness slots use a reserved-zero encoding. Raw data must be
structurally validated before a witness is installed.
Equal scoped witnesses permit the existing declared-identity fast path;
unequal declared witnesses are an identity mismatch even when their payload
structures happen to match. Work type witnesses relocate through a type
forwarding/interner map; Main type witnesses remain unchanged. `narrow` will
use the same scoped-ID encoding.

`narrow` is zero in this RFC series. Its future interpretation must not change
the exact type identity in `ty`; it will describe additional proven
trait/interface refinement, not another nominal type.

## Compatibility

This is an internal runtime migration. Telora surface values, diagnostics,
JSON, module publication, native callback behavior, and successful CLI output
do not change. The legacy owned `Value` remains available at explicit Host/API
boundaries but is not used between internal evaluation stages.

## Child RFCs

- RFC 0246: fixed layout, flat Meta, traits, and size regressions.
- RFC 0247: inline text, World-relative references, and immediate NativeType.
- RFC 0248: canonical witness registry and removal of declared value wrappers.
- RFC 0249: copy collector and boundary migration, correctness and performance
  evidence, and legacy cleanup.

## Acceptance criteria

1. every VM register and Heap edge is a 32-byte `Val`;
2. source provenance survives calls, publication, and boundary projection;
3. common physical classification uses Meta masks without Heap traversal;
4. short text and NativeType values allocate no Heap object;
5. narrowing changes only witness metadata after validation;
6. Work/Main and Work/Work copy retain graph sharing and recursive edges;
7. strict and best-effort evaluation preserve their current outcomes; and
8. workspace tests, clippy, and recursive performance fixtures pass.
