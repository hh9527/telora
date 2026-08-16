# RFC 0236: Struct and Enum declaration initializers

- Status: Implemented
- Tracking issue: #85
- Depends on: RFC 0027, RFC 0028, RFC 0218, RFC 0235

## Summary

Telora adds contextual `struct` and `enum` initializers to `type`
declarations:

```telora
type User = struct {
    id: Int,
    name: String,
};

type OptionValue(Item) = enum {
    'None,
    'Some(Item),
};
```

This RFC establishes the final declaration shape while deliberately preserving
the current structural TypeMetadata semantics. It lowers the new forms to the
existing normalized `struct(ctx, fields)` and `enum(ctx, variants)` operations.
Distinct equal-shaped declarations therefore remain structurally assignable
until RFC 0237 adds declared identity end to end.

`struct` and `enum` are contextual keywords only in the direct initializer
position of a `type` declaration. The legacy callable prelude bindings and
`@struct` / `@enum` decorators remain temporarily available so this RFC can be
implemented, tested, and committed without forcing the complete repository
migration. RFC 0240 removes those legacy surfaces after identity, recursion,
and parameterized-family semantics are complete.

## Motivation

The current recommended surface expresses data-model kind through an ordinary
decorator:

```telora
@struct
type User = {
    id: Int,
    name: String,
};

@enum
type OptionValue(Item) = {
    None: 'None,
    Some: Item,
};
```

This made sense while `struct` and `enum` were purely structural metadata
normalizers. RFC 0235 assigns declared Struct and Enum roots language-owned
identity and reserve/seal behavior. An ordinary decorator Function cannot own
that lifecycle.

The new surface also makes Enum declarations agree with their values and
patterns:

```telora
'None
'Some(value)

match option {
    'None => ...,
    'Some(value) => ...,
}
```

The declaration should describe the same tagged alternatives rather than expose
the internal descriptor map encoding.

## Scope

This RFC adds:

1. direct Struct and Enum initializer grammar after `type Name[...] =`;
2. tagged Enum variant declarations;
3. CST recovery and source ranges for both forms;
4. AST lowering to existing normalized metadata operations;
5. parameterized family support through existing `type Name(T)` binders;
6. type, field, and variant decorator preservation; and
7. focused diagnostics and semantic-query coverage.

This RFC does not add:

- nominal assignability or declared value brands;
- a new TypeMetadata descriptor kind;
- recursive behavior beyond the existing concrete type path;
- parameterized recursion;
- positional Structs, newtypes, or unit Structs;
- variadic Enum payloads;
- separate generic-type application syntax; or
- removal of the legacy constructors and decorators.

## Grammar

The declaration grammar becomes conceptually:

```text
type_binding:
    decorator* 'type' Identifier [type_parameters]
    '=' type_initializer ';'

type_initializer:
    struct_initializer
  | enum_initializer
  | expression

struct_initializer:
    'struct' '{' [struct_field (',' struct_field)* [',']] '}'

struct_field:
    decorator* Identifier ':' expression

enum_initializer:
    'enum' '{' [enum_variant (',' enum_variant)* [',']] '}'

enum_variant:
    decorator* Atom ['(' expression ')']
```

`struct` and `enum` are recognized as initializers only when they occur
immediately after the declaration `=` and are followed by `{`. During this
transition RFC, ordinary expressions such as `struct('None, fields)` continue
to parse as calls to the existing prelude Function.

The initializer is not a general expression:

```telora
let metadata = struct {value: Int}; # invalid
def build = fn() { enum {'None} };  # invalid
```

The final RFC 0240 may reserve both words lexically after their ordinary value
bindings are removed. That removal is not required for this parser boundary.

## Struct fields

The initial Struct form accepts named fields only:

```telora
type Request = struct {
    id: String,

    @codec_format('Base64)
    body: Bytes,
};
```

Each field requires an explicit TypeMetadata expression. Field punning and
spread fields are rejected because a declared metadata shape must be complete
at its declaration site.

Field decorators retain their current RFC 0025 context:

```text
{kind: 'Field, name: field_name}
```

They run during the ordinary metadata initializer exactly once and before the
Struct root is normalized.

## Enum variants

A bare Atom declares a unit variant:

```telora
type State = enum {
    'Pending,
    'Ready,
};
```

An Atom followed by one parenthesized TypeMetadata expression declares one
payload:

```telora
type ResultValue(Value, Error) = enum {
    'Ok(Value),
    'Err(Error),
};
```

The payload arity is exactly zero or one. Multiple positional payload values
use one explicit Tuple metadata value:

```telora
type Event = enum {
    'Moved(Tuple([Int, Int])),
};
```

Named payloads use a declared Struct metadata value. This RFC does not give
`'Moved(Int, Int)` a hidden Tuple meaning.

Variant decorators use the existing Field decorator context with the unquoted
tag name:

```telora
type WireResult(Value) = enum {
    @codec_format('Untagged)
    'Ok(Value),
    'Missing,
};
```

The exact standard codec vocabulary remains owned by its existing RFCs; this
RFC only preserves decorator evaluation and source ownership.

Duplicate Struct field names and duplicate Enum tags are errors at the second
declaration. An empty Struct remains valid if the current normalized Struct
constructor accepts it. An empty Enum retains the current constructor error.

## Lowering

For:

```telora
@root_attribute(...)
type User = struct {
    @field_attribute(...)
    id: Int,
};
```

the semantic lowering is equivalent to:

```telora
@root_attribute(...)
type User = struct(
    {kind: 'Type, name: "User"},
    {
        @field_attribute(...)
        id: Int,
    },
);
```

The first `struct` in the source is syntax; the second in the conceptual
lowering denotes the existing internal model operation. Source code does not
contain or observe the synthetic call node as authored text.

Enum lowering converts its tagged declaration surface to the existing
normalized map:

```telora
type OptionValue(Item) = enum {
    'None,
    'Some(Item),
};
```

```text
enum(
    {kind: 'Type, name: "OptionValue"},
    {
        None: 'None,
        Some: Item,
    },
)
```

The unit marker is the existing metadata-level `'None`; it is not the declared
variant's value and does not add a second runtime representation.

Root decorators wrap the normalized initializer in the same order as today.
The model initializer runs before an outer root decorator, matching:

```telora
@outer
@struct
type Old = fields;
```

The lowered expression uses the initializer's complete source range for the
model call, the authored field/variant range for member metadata, and the type
name range for the synthetic context name. Diagnostics must not point to a
nonexistent generated source string.

## Parameterized families

Existing RFC 0218 binders remain unchanged:

```telora
type Box(Item) = struct {
    value: Item,
};
```

The initializer is evaluated once with the same rigid Bound metadata used by
the current decorated body. It publishes the same structural symbolic template
and callable scheme as the equivalent legacy declaration:

```text
Box : for(Item) Fn(TypeOf(Item)) -> TypeOf(Box(Item))
```

Application remains:

```telora
Box(Int)
```

This RFC does not add `Box[Int]`, per-application body execution, partial
application, or recursive family applications.

## CST recovery

The lossless CST retains distinct `StructInitializer`, `EnumInitializer`, and
`EnumVariant` nodes. Their delimiters, comments, commas, decorators, and error
nodes remain queryable even when lowering cannot produce an executable AST.

Recovery must provide focused diagnostics for:

- missing `{` or `}`;
- a Struct field without `:` or a metadata expression;
- an Enum member that is not an Atom;
- a payload `(` without one complete expression or `)`;
- duplicate field or variant names; and
- a missing declaration semicolon.

A damaged member does not erase unaffected sibling members or later top-level
bindings from the recovered workspace. Strict parsing and execution still fail
until the declaration is complete.

## Static and runtime semantics

After lowering, all existing analysis and VM behavior remains authoritative.
In this RFC:

```telora
type Left = struct {value: Int};
type Right = struct {value: Int};
```

`Left` and `Right` remain structurally assignable. The new syntax does not
smuggle nominal identity into `Named`, HIR definition IDs, synthetic attributes,
or display names.

Recursive concrete declarations continue through the existing up-link and
promotion path. Parameterized recursion continues to be rejected under RFC
0232. `show`, LSP, codec, schema, `Dyn`, and TypeDesc observe the same normalized
metadata graph as the equivalent legacy declaration.

## Migration boundary

During RFC 0236 through RFC 0239, both surfaces are accepted:

```telora
@struct type Old = {value: Int};
type New = struct {value: Int};
```

Focused migration should prefer the new form in new tests and fixtures, but a
repository-wide mechanical rewrite is deferred to RFC 0240. Historical RFCs
retain their original source examples.

RFC 0240 removes without compatibility aliases:

- `@struct` and `@enum` declaration decorators;
- ordinary prelude Functions named `struct` and `enum`; and
- explicit source calls such as `struct('None, fields)`.

Code that genuinely computes structural metadata dynamically must be audited
rather than mechanically converted to a declared nominal type.

## Acceptance criteria

1. record Struct declarations parse, lower, execute, and expose the same
   normalized metadata as their legacy equivalent;
2. unit and single-payload Enum declarations lower to the existing canonical
   Enum descriptor and validate representative values;
3. parameterized Struct and Enum declarations preserve RFC 0218 schemes and
   `Family(A)` application;
4. root, field, and variant decorators execute once in the established order
   with the established contexts;
5. direct and mutual concrete recursion retain existing behavior;
6. distinct equal-shaped declarations remain structurally assignable in this
   RFC;
7. CST nodes and semantic queries retain accurate authored ranges;
8. incomplete initializers produce focused diagnostics while later bindings
   remain recoverable;
9. legacy decorator and callable surfaces continue to pass until RFC 0240;
10. no `Box[A]`, positional Struct, newtype, nominal identity, or recursive
    family behavior is introduced; and
11. parser, syntax, type-analysis, module, CLI, and LSP regressions pass.

## Implementation plan

1. add contextual initializer productions and CST node wrappers;
2. lower Struct fields and tagged Enum variants to ordinary metadata
   expressions with authored source ranges;
3. synthesize the current model context and operation before root decorators;
4. add duplicate/member/recovery diagnostics;
5. cover ordinary, decorated, parameterized, recursive, malformed, and legacy
   declarations; and
6. run formatting, warning-denied Clippy, and the full workspace test suite.

## Rejected alternatives

### Add lexically reserved keywords and remove old calls immediately

This couples parser work to the entire ecosystem migration and prevents an
independently executable child RFC. Contextual recognition provides the final
declaration spelling while RFC 0240 owns the intentional removal.

### Lower directly to a compiler-only TypeDescriptor

That would bypass the authoritative TypeMetadata VM and make the syntax path
semantically different from decorators, codecs, schema, and user-space
interpreters. This RFC intentionally proves the surface against the existing
metadata construction path first.

### Keep the internal Enum map as source syntax

`{Some: Item}` describes descriptor storage rather than the language's tagged
value shape. Direct `'Some(Item)` declarations align types, values, and
patterns and leave the normalized map internal.

### Add nominal identity in the same implementation

Parser correctness and identity correctness have different failure modes. RFC
0237 is the explicit go/no-go boundary for values, worlds, dynamic erasure, and
codecs; this RFC must remain a structural surface migration.
