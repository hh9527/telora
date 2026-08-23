# RFC 0259: Member Properties and Indexed Reflection

- Status: Implemented
- Depends on: RFC 0258
- Supersedes: the single `@property` carrier marker in RFC 0258

## Summary

Typed properties extend from nominal types to Struct fields and Enum variants.
Members remain coordinates inside their owning canonical type rather than new
runtime identity objects. The complete property key space is:

```text
Ty(TypeId, PropertyTypeId)
Field(TypeId, member_index, PropertyTypeId)
Variant(TypeId, member_index, PropertyTypeId)
```

`member_index` is zero-based and follows the canonical member-name order in the
sealed descriptor. Reordering source declarations does not change an index;
renaming a member does.

The logical property record set is append-only. A later record with the same
key shadows the earlier record. The implementation materializes only the
effective head for each key and may overwrite its table entry after the new
record has been evaluated successfully.

## Decorator fold protocol

Ordinary decorators use one category-specific context and the preceding value
for their exact property key:

```telora
Fn(TypeDesc, Option(P)) -> P
Fn(FieldPropertyCtx, Option(P)) -> P
Fn(VariantPropertyCtx, Option(P)) -> P
```

Configured decorators return the same provider forms. For example:

```telora
deco(arguments): Fn(FieldPropertyCtx, Option(P)) -> P
```

For decorators written in lexical order:

```telora
@f1
@f2
type A = struct {};
```

where both providers return `P`, evaluation is:

```text
p1 = f1(A, None)
p2 = f2(A, Some(p1))
effective[Ty(A, P)] = p2
```

The previous head is not removed before provider evaluation. A failed provider
does not change the effective table. Property-specific policy belongs in the
provider: it may merge, replace, or reject a preceding value.

Property carriers declare their permitted owner categories with one privileged
capability marker:

```telora
@property('Type)
type TypeOnly = struct {};

@property('StructType)
type StructOnly = struct {};

@property('Type)
@property('Member)
type Both = struct {};
```

The supported capabilities are `Type`, `StructType`, `EnumType`, `Member`,
`Field`, and `Variant`. `Type` covers both nominal Struct and Enum owners;
`Member` covers both Field and Variant owners. The narrower capabilities admit
only their exact category. Multiple annotations OR their capability bits.

Core bootstraps one nominal carrier:

```telora
@property('Type)
type PropertyAttr = struct { bits: Int };
```

The marker records `Ty(P, PropertyAttr) -> PropertyAttr { bits }`.
`PropertyAttr` bootstraps its own TypeId and marker record. `@property` is a
reserved intrinsic, runs for every local carrier before ordinary providers are
validated, and does not use the fold ABI. The physical representation is a
`u32` bit set even though the public bootstrap record uses `Int`.

## Contexts

The standard property module exports nominal read-only context types:

```telora
type FieldPropertyCtx = struct {
    owner: Type,
    index: Int,
    name: String,
    ty: Type,
};

type VariantPropertyCtx = struct {
    owner: Type,
    index: Int,
    name: String,
    payload: Option(Type),
};
```

The system owns the authoritative `(TypeId, index)` target. Providers receive
read-only context data and cannot redirect the property by changing or returning
that data.

## Evaluation order and visibility

A concrete nominal declaration is evaluated as one transaction:

1. seal its property-independent Type descriptor and canonical TypeId;
2. establish local `@property(...)` capability records;
3. evaluate every field and variant decorator chain;
4. freeze the complete effective member-property snapshot;
5. evaluate every ordinary type decorator chain against that snapshot; and
6. publish the effective member and type heads atomically.

Member decorators do not observe partial properties from other members in the
same declaration. Type decorators observe every completed member property in
the declaration, but do not observe partial type-property chains except through
their explicit `previous` argument. Reading a member property never consumes or
shadows it; only a successful write to the exact same key shadows a head.

The Type descriptor is available in the construction transaction before any
property is evaluated. It becomes externally visible only when its enclosing
module publication succeeds.

## Typed property queries

The explicit-witness API is:

```telora
prop.get_type_prop(Target, P) -> Option(P)
prop.get_field_prop(Target, index, P) -> Option(P)
prop.get_variant_prop(Target, index, P) -> Option(P)
```

The corresponding generic native contracts are:

```telora
for(P) Fn(Type, TypeOf(P)) -> Option(P)
for(P) Fn(Type, Int, TypeOf(P)) -> Option(P)
for(P) Fn(Type, Int, TypeOf(P)) -> Option(P)
```

A future general implicit witness rule may expose:

```telora
prop.get_type_prop@[P](Target)
prop.get_field_prop@[P](Target, index)
prop.get_variant_prop@[P](Target, index)
```

The public query returns the materialized effective head. Property history is
not a public runtime value.

## Indexed reflection

`std/type-desc` exposes canonical member descriptions:

```telora
type FieldDesc = struct { index: Int, name: String, ty: Type };
type VariantDesc = struct { index: Int, name: String, payload: Option(Type) };

type_desc.fields(Type) -> Array(FieldDesc)
type_desc.variants(Type) -> Array(VariantDesc)
```

`std/dyn` projects values with the same indices:

```telora
dyn.get_field_value(Dyn, index) -> Dyn
dyn.get_variant_index(Dyn) -> Int
dyn.get_variant_payload(Dyn, index) -> Option(Dyn)
```

The descriptor carried by `Dyn` is authoritative. A non-Struct field access, a
non-Enum variant access, an out-of-range index, or a variant-index mismatch is a
sourced runtime failure. A valid unit variant returns `None`; a valid payload
variant returns `Some(Dyn)`.

The projected `Dyn` carries the member descriptor from the canonical owner, so
reflection never guesses from a structurally similar nominal type.

## Runtime representation

The property table is an ordered effective mapping:

```text
PropertyKey -> Val
```

A BTree map or sorted vector is semantically equivalent. Replacement occurs
only after provider evaluation and runtime property-type validation succeed.
Publication copies only effective property values reachable from the sealed
table; shadowed intermediate values are not runtime roots unless retained by a
new effective value.

## Scope

This RFC supports member decorators only on concrete nominal Struct and Enum
declarations. It does not define member properties on structural aliases,
generic type-constructor templates, or concrete applications of decorated type
families. Quote, generated code, traits, and member-property history inspection
remain deferred.

## Acceptance criteria

- the three property-key categories cannot collide;
- provider results require a carrier capability compatible with the exact owner;
- canonical member indices are deterministic and source-order independent;
- same-key decorators fold in lexical order and publish only the final head;
- failed folds leave no partial property publication;
- all member properties are available to type decorators, and member reads are
  non-destructive;
- field and variant contexts contain the canonical owner, index, name, and
  member type information;
- typed queries return the effective property by canonical type identity and
  member index;
- indexed Dyn reflection returns correctly typed Dyn children and rejects
  invalid owner kinds, indices, and variant mismatches;
- recursive nominal types retain finite indexed reflection; and
- the complete workspace test suite passes.
