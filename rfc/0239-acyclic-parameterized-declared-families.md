# RFC 0239: Acyclic parameterized declared families

- Status: Proposed
- Tracking issue: #85
- Depends on: RFC 0218, RFC 0232, RFC 0235, RFC 0237, RFC 0238

## Summary

Telora makes an acyclic parameterized `struct` or `enum` declaration a
TypeMetadata family with one private declaration-head identity and one
canonical declared identity for every complete application.

```telora
type Box(Item) = struct {value: Item};
type Result(Value, Error) = enum {'Ok(Value), 'Err(Error)};

type IntBox = Box(Int);
```

`Box` remains an ordinary TypeMetadata Function. Calling `Box(Int)` performs
capture-avoiding substitution in its symbolic template and returns a declared
TypeMetadata object. It does not execute arbitrary source code or introduce a
second generic-type mechanism.

## Identity

An application identity is the pair:

```text
(provider declaration head, canonical TypeMetadata arguments)
```

Therefore:

- repeated `Box(Int)` applications denote the same type;
- aliases of `Box(Int)` preserve that identity;
- imported and reexported `Box` values preserve the provider's family head;
- `Box(Int)` and `Box(String)` are distinct;
- `Box(Int)` and an equal-shaped `Other(Int)` are distinct; and
- nested applications use the canonical identities of their arguments.

Identity never derives from a display string, an expanded structural body, a
consumer-local binding, or heap allocation order. The implementation uses a
private canonical descriptor key that is stable under copying and equivalent
descriptor reconstruction. The key is not exposed through Telora syntax,
TypeDesc, Dyn, schema, codecs, or diagnostics.

## Family template and application

RFC 0218 remains authoritative for family analysis. The declaration body is
evaluated once with rigid Bound descriptors. A complete application:

1. validates arity and canonical TypeMetadata arguments;
2. substitutes every Bound occurrence without capture;
3. computes the application identity from the provider head and arguments;
4. wraps the substituted Struct or Enum body in that declared identity;
5. validates that no Bound or pending recursive edge remains; and
6. publishes or reuses the canonical application.

Equivalent applications may produce distinct heap handles, but equality,
assignability, value ownership, codec witnesses, and world copying observe the
same `DeclaredTypeId`.

Partial application is not added. A parameterized declaration can be passed as
a Function value, but applying it requires its complete declared arity.

## Static checking and values

The result of a family application behaves exactly like a concrete declared
type under RFC 0237:

```telora
type Box(Item) = struct {value: Item};
let a: Box(Int) = {value: 1};
let b: Box(String) = {value: "x"};
```

Expected Struct literals and Enum constructors receive the application owner.
Projection and pattern matching expose the substituted body. Assignability
requires the same application identity; equal shape is insufficient.

Aliases name an existing application and never mint a new declaration:

```telora
type A = Box(Int);
type B = Box(Int);
```

`A`, `B`, and a direct `Box(Int)` application denote the same type.

## Modules and worlds

Exporting a family exports its provider declaration head together with its
symbolic template. Import aliases and reexports retain that head. A consumer
must not rebind the family to a consumer-local identity.

Work-to-Main and Work-to-Work copying preserves family heads, canonical
argument keys, application owners, and sharing. Existing MainWorld argument
edges remain in MainWorld. Copying does not expand application identity into
the structural body or recompute it from rendered metadata.

## Dyn, TypeDesc, codecs, and schema

Dyn retains the exact application descriptor and declared value. TypeDesc
exposes the substituted public Struct or Enum body but not the private family
head or argument key.

Codec planning retains the application owner. Decode wraps the result with
that owner; encode rejects a value owned by a different application even when
the payload shape is equal. Schema generation may share deterministic
definitions for repeated occurrences of the same application and must keep
distinct declarations distinct.

## Recursive families

Parameterized recursion remains rejected under RFC 0232, including direct,
mutual, and mixed concrete/family cycles:

```telora
type List(Item) = struct {
    head: Item,
    tail: Option(List(Item)),
};
```

No application may be approximated as `Any`, structurally truncated, or
published with an unresolved Bound. Supporting recursive applications requires
a separate canonical instantiation-graph design.

## Diagnostics and resource bounds

Diagnostics use authored family and argument names and do not reveal private
keys. Arity errors, non-Type arguments, unresolved Bounds, invalid initializer
kinds, and recursive-family rejection are deterministic.

Template substitution and canonical-key construction are linear in the
reachable acyclic argument and body graphs, modulo deterministic map costs.
Traversal uses visited sets and does not repeatedly compare expanded recursive
bodies. Existing fuel and allocation accounting applies.

## Acceptance criteria

This RFC is complete when:

1. parameterized Struct and Enum declarations produce declared applications;
2. repeated equivalent applications have equal identity;
3. different arguments and different family heads have different identities;
4. aliases do not mint identity;
5. import aliases and reexports preserve provider-head identity;
6. nested applications retain every owner and substituted descriptor;
7. expected literals, projection, matching, Dyn, TypeDesc, codec, and schema
   obey concrete declared semantics;
8. Work/Main and Work/Work transfer preserves applications and sharing;
9. direct, mutual, and mixed recursive families remain rejected; and
10. canonical identity does not depend on debug or display formatting.

## Non-goals

This RFC does not define:

- parameterized recursion;
- partial TypeMetadata family application;
- higher-kinded parameters;
- public identity reflection or casting;
- positional Struct/newtype declarations; or
- compatibility with `@struct`, `@enum`, or callable model constructors.
