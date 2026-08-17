# RFC 0247: Inline Text and World References

- Status: Proposed
- Tracking issue: #88
- Depends on: RFC 0245, RFC 0246

## Summary

Telora will give short String and Atom values a canonical allocation-free
encoding, distinguish Main and Local text and Heap references in Meta, and
encode NativeType identity directly in `raw`.

## Inline text

For `IString` and `IAtom`, `raw` is eight bytes:

```text
bytes 0..7  UTF-8 content, zero-filled after the content
byte  7     content length, 0 through 7
```

A String or Atom whose UTF-8 encoding is at most seven bytes must use the
inline form. Longer values must use the corresponding Main or Local intern
reference. This unique representation rule prevents equal short text from
having both inline and referenced forms. Embedded zero bytes are valid because
the length is explicit.

All built-in Atoms (`None`, `Some`, `Ok`, `Err`, `True`, and `False`) fit in the
inline representation.

## Reference kinds

`MainString`, `MainAtom`, `MainHeap`, and `MainUplink` are stable background
references. `LocalString`, `LocalAtom`, `LocalHeap`, and `LocalUplink` belong to
the active WorkWorld. Their raw payload is a slot or packed handle whose exact
format is private to the owning arena.

Local text is copied by resolving its content and interning that content in the
target. Local Heap objects are copied through the object forwarding map.
Inline and Main values are retained unchanged when the target shares the same
MainWorld.

## NativeType

`NativeType` is immediate:

```text
raw[0..32]  native module ID
raw[32..64] module-local type ID
```

Display names and Host payload behavior are resolved through the registered
native-type table. Native opaque values remain Heap objects; only their type
identity becomes immediate.

## Acceptance criteria

1. every String and Atom of at most seven UTF-8 bytes allocates no text entry;
2. longer text resolves correctly from both Main and Local tables;
3. equal text compares and hashes equally across World boundaries;
4. Work/Work and Work/Main publication canonicalize Local text by content;
5. NativeType construction and comparison allocate no Heap object;
6. native opaque downcasts retain exact module/local identity checks; and
7. codecs, formatting, diagnostics, and module tests pass.

