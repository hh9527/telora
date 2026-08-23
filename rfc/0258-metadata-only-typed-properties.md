# RFC 0258: Metadata-Only Typed Properties

- Status: Implemented
- Supersedes: decorator transformation and string-keyed decorator attributes in RFC 0025, RFC 0026, RFC 0030, and RFC 0036
- Depends on: RFC 0235, RFC 0237, RFC 0248, RFC 0249

> RFC 0259 supersedes this RFC's single marker, one-argument provider ABI,
> duplicate-key rejection, query names, and type-only key space. This document
> remains the historical rationale for the metadata-only transition.

## Summary

Telora changes a decorator from a function that replaces `TypeMetadata` into a
function that publishes one typed property about an already constructed nominal
type. A property is indexed only by the canonical identities of its target and
property types:

```text
(TypeId(Target), TypeId(Property)) -> PersistentValue
```

For example:

```telora
@property
type DisplayBy = struct {
    template: String,
};

def display_by: Fn(String) -> Fn(TypeDesc) -> DisplayBy = fn(template) {
    fn(target) {
        let property: DisplayBy = {template};
        property
    }
};

@display_by("{host}:{port}")
type Endpoint = struct {
    host: String,
    port: Int,
};
```

The `Endpoint` descriptor is constructed first and retains exactly the same
shape it would have without `@display_by`. The configured provider then receives
that sealed descriptor, returns a `DisplayBy`, and the tool stage publishes the
property under `(Endpoint, DisplayBy)`. Interpreters continue to interpret the
property at runtime; this RFC does not introduce quote or generated code.

## Property types and bootstrap

`NativeDecorator` is the built-in marker property type. Core pre-seeds its own
marker, which establishes the root of the property-type relation:

```text
property(NativeDecorator, NativeDecorator) = marker
```

The intrinsic decorator `@property` is permitted only on a concrete nominal
`struct` or `enum` declaration with no type parameters. It publishes:

```text
property(Prop, NativeDecorator) = marker
```

A normal decorator result is admissible only when all of these conditions hold:

- its runtime value carries a canonical nominal `Val.ty`;
- the inferred result is that same concrete nominal type;
- the type is marked with the `NativeDecorator` property; and
- the type is neither a primitive, `Any`, `Dyn`, a structural collection,
  unresolved `Bound`, nor a type constructor template.

Ordinary nominal `struct` and `enum` carriers are sufficient. Transparent
newtype syntax such as `type Prop = struct(Int)` is not a prerequisite and does
not change the future registry contract.

`@property` is a reserved intrinsic, not an imported or shadowable function.
Its bootstrap behavior is the only privileged property publication path.

## Decorator ABI

The configured form:

```telora
@deco(arguments)
type Target = struct { ... };
```

requires:

```telora
deco(arguments): Fn(TypeDesc) -> P
```

The unconfigured form:

```telora
@deco
type Target = struct { ... };
```

requires:

```telora
deco: Fn(TypeDesc) -> P
```

`P` must be a valid property type. The supplied `TypeDesc` is a read-only view;
the system owns the authoritative target `TypeId`. A provider cannot redirect a
property to another target by returning or modifying metadata.

Multiple different property types on one target are unordered. Publishing the
same `(target, property type)` twice is an error. All providers on a declaration
are evaluated and validated before any of their properties become visible.
Property evaluation shares the enclosing tool-stage quota and reports the
decorator source location on failure.

## Typed query and interpreters

The standard typed query passes the property type as its runtime witness:

```telora
type_property.get(P, Target) -> Option(P)
```

Telora does not currently inject `TypeOf(P)` witnesses for explicit type
application. The query therefore uses the same explicit-witness convention as
other generic native functions instead of adding property-specific call sugar.

It obtains the property type from the explicit type argument and looks up the
canonical target `TypeId`. No public or internal query uses a string key such as
`"std/fmt.display"`.

Existing type-directed interpreters migrate as follows:

- `fmt.display_by` publishes a nominal display-template property;
- `regex.parse_by` publishes a nominal parse-regex property;
- `string.decode_by_parse` and `string.encode_by_display` publish nominal codec
  properties; and
- retained JSON type-level configuration publishes nominal JSON properties.

The interpreter may cache a validated execution plan, but the property remains
the semantic source of truth.

## Type-level scope and codec cleanup

This RFC deliberately has no member identity. Decorators are accepted only on
concrete nominal type declarations. Decorators on struct fields, enum variants,
Dict fields, aliases, values, functions, and generic type constructors are
rejected.

The legacy JSON member decorators are removed without compatibility behavior:

```text
json.rename
json.flatten
json.default
json.skip_serializing_if
```

Their field/variant planning, encode/decode, schema, tests, and maintained
documentation are removed. Type-level `json.rename_all` and `json.untagged`
remain and migrate to typed properties. A future member-property RFC must first
define a stable `MemberId`; it must not infer member identity from the member's
value type.

## Runtime ownership and atomicity

Property values are evaluated in the existing tool Work world. After every
provider for one target succeeds, their roots are copied once into the building
Main world and installed in its property registry. The registry stores Main
values only. Runtime lookup is therefore a cheap pair-of-`u32` lookup and never
copies a property between Host and World.

Properties follow normal Main/Work tracing and publication rules. A property
containing a failed node cannot be published. A module failure exposes no
partially installed registry from that module. Importing a sealed module does
not rerun its decorators.

## Deferred work

This RFC does not add:

- quote, decorator code generation, or interpreter specialization;
- trait declarations, trait dictionaries, or `narrow` witnesses;
- transparent newtypes;
- generic property types or decorated type families;
- member, field, or variant properties; or
- compatibility adapters for transformed `TypeMetadata` or string attributes.

## Implementation plan

1. Stop parser decorator transformation, retain intrinsic struct/enum metadata
   construction, and reject decorators outside concrete nominal type bindings.
2. Add `NativeDecorator`, `@property`, Main-owned typed-property storage, and
   typed lookup with duplicate and ownership validation.
3. Evaluate decorators after their target descriptor is sealed; validate the
   provider ABI and nominal result, then publish each target's batch atomically.
4. Migrate fmt, regex, string codec hooks, and retained JSON type decorators to
   nominal properties and typed lookup.
5. Remove legacy JSON member decorators and their plan, transform, schema, test,
   guide, and SSOT surface.
6. Add parser, type-stage, module-boundary, duplicate, invalid-property, and
   interpreter regressions; format and run the complete workspace suite.

## Acceptance criteria

- a decorator cannot alter the target's canonical descriptor or `TypeId`;
- `@property` accepts only concrete nominal struct/enum declarations;
- decorator providers receive the sealed target `TypeDesc` and must return a
  marked concrete nominal value with matching static and runtime identity;
- duplicate `(target, property type)` publication fails at the second decorator;
- `type_property.get(P, Target)` returns `Some(P)` or `None` by TypeId identity; future
  `get_type_property@[P](Target)` follows a general implicit `TypeOf(P)` witness rule;
- fmt, regex, string codec hooks, `json.rename_all`, and `json.untagged` work
  without string-keyed decorator attributes;
- member-level JSON decorators are absent from API, implementation, docs, and
  tests; and
- the complete workspace test suite passes.
