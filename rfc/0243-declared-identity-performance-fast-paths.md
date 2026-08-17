# RFC 0243: Declared-Identity Performance Fast Paths

- Status: Implemented
- Tracking issue: #87
- Depends on: RFC 0241, RFC 0242, RFC 0237

## Summary

Telora will treat an existing declared owner as an unforgeable runtime witness.
When an expected declared descriptor and a value owner have equal
`DeclaredTypeId`, validation succeeds without recursively revalidating the
payload. Raw values still undergo full structural validation before the
validator creates that wrapper.

Static comparison likewise stops at equal declared identity. Generic family
arguments are compared or unified directly; the structural body is traversed
only where type variables or an explicitly anonymous structural operation
requires it.

## Runtime trust boundary

For expected declared type `D`:

```text
validate(D, declared(owner = D, payload = p)) = Ok(existing value)
validate(D, declared(owner = E, payload = p)) = Err(identity mismatch)
validate(D, raw p) = structural_validate(body(D), p); brand(D, p)
```

The first rule is sound because surface Telora cannot construct a declared
wrapper or its owner token. Wrappers are created only by expected-type
construction, declared constructors, typed codec/Host adaptation, or
`validate`. Each creation site must either start from already trusted typed
data or perform the structural check once.

The fast path is transitive: a declared child embedded in a larger raw value
may short-circuit, while the raw enclosing structure is still checked.

## Static fast paths

The type checker observes these rules before deep resolution:

- equal concrete declared IDs are assignable immediately;
- different concrete declared IDs are incompatible even if bodies match;
- applications of the same declared family head unify their argument list;
- display names and structural bodies are not comparison keys; and
- legacy `Named` aliases may still be exposed, but a resolved declared root is
  not recursively cloned merely to compare identity.

Deep substitution remains required when a parameterized declared body is being
instantiated, projected, decoded, or otherwise structurally observed.

## Boundary audit

Implementation audits every constructor of `DeclaredValue` and
`Object::Declared`. No public native function may accept an arbitrary owner and
payload pair. Heap copy and publication preserve wrappers but do not create
new identity. Codec and Host boundaries must validate raw data before wrapping
unless their adapter contract already guarantees the exact declared type.

## Acceptance criteria

1. matching declared validation does not inspect the payload body;
2. raw validation still detects an invalid deeply nested field;
3. structurally equal but separately declared owners remain incompatible;
4. a successful raw validation returns the expected declared wrapper;
5. declared type comparison uses ID or family arguments without body-sized
   `Debug` keys;
6. the fast paths improve declared comparison independently of runtime value
   storage representation; and
7. all codec, Dyn, schema, module, and Host-boundary tests pass.
