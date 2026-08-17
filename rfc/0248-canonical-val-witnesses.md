# RFC 0248: Canonical Val Witnesses

- Status: Implemented
- Tracking issue: #88
- Depends on: RFC 0245, RFC 0246, RFC 0247, RFC 0237, RFC 0243

## Summary

Telora will replace runtime `DeclaredValue` and `Object::Declared` wrappers
with a canonical `u32` arena slot stored directly in `Val.ty`.
Validation of raw values remains structural; validation of an already matching
witness remains an identity fast path.

## Witness registry

Each World owns a canonical type arena from declared identity to `TypeId`.
Meta records whether `Val.ty` addresses the Main arena, the current Local
arena, or no arena. The arenas include concrete declarations and applications
of parameterized declared families. IDs are not serialized as a language or
Host ABI.

The registry retains the full identity key: module identity, declaration slot,
and canonical argument identities. Display names and structural body strings
are not keys.

## Narrow operation

```text
narrow(expected, raw value):
    structurally validate expected.body against value
    return value with ty = expected.id

narrow(expected, value with witness expected.id):
    return value unchanged

narrow(expected, value with another declared witness):
    fail with declared identity mismatch
```

Installing a witness preserves `loc`, Meta, raw payload, and graph sharing. It
does not allocate, copy, or wrap the payload. Since `Val` is copied by value,
different edges may hold the same raw payload with different valid narrowing
metadata only where the language's existing validation semantics permit that.

Local witnesses relocate through a type forwarding/interner map during
Work/Work or Work/Main copy. Main witnesses remain unchanged when the target
shares the MainWorld.

## Type values versus witnesses

A runtime value whose Heap sub-kind is `Type` is first-class TypeMetadata. A
`ty` witness is proof about an ordinary value. The two are related through
the canonical registry but are not interchangeable representations.

## Acceptance criteria

1. no internal runtime `DeclaredValue` or `Object::Declared` remains;
2. raw invalid payloads fail before acquiring a witness;
3. matching witnesses avoid structural payload traversal;
4. different declared identities fail despite equal structure;
5. recursive and parameterized declared identities receive canonical IDs;
6. copying a Val preserves or relocates its witness according to context
   lifetime without structural revalidation; and
7. declared Struct/Enum, codec, Dyn, formatting, and Host-boundary tests pass.

## Outcome

Declared payload wrappers were removed. `Val.ty` now stores an optional scoped
reference to a `DeclaredType` witness while the payload remains directly in
`Val.meta` and `Val.raw`. Narrowing preserves the payload and source location,
Host export reconstructs `Value::Declared` only at that explicit boundary,
and copy collection relocates Work witnesses through the same object graph as
ordinary Heap references.

Logical equality resolves witness slots to declared identities before
comparing raw payloads. This also handles equivalent identities represented by
different transient metadata handles and preserves the existing declared-Atom
comparison with its corresponding raw Atom.
