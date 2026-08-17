# RFC 0240: Declared-model migration and legacy removal

- Status: Implemented
- Tracking issue: #85
- Depends on: RFC 0235, RFC 0236, RFC 0237, RFC 0238, RFC 0239

## Summary

Telora completes the declared Struct/Enum phase by making contextual
declaration initializers the only public named-model surface:

```telora
type User = struct {id: Int, name: String};
type OptionValue(Item) = enum {'None, 'Some(Item)};
```

The following legacy surfaces are removed without compatibility aliases:

```telora
@struct type User = {id: Int};
@enum type State = {Idle: 'None};
struct(context, fields)
enum(context, variants)
```

The compiler and VM may retain private normalization operations used by the
contextual initializers. They are implementation details, are absent from the
prelude and module interfaces, and cannot be imported, captured, called, or
reexported by Telora source.

This RFC also migrates all active code, fixtures, standard modules, current
experiments, SSOT, and tutorials to declared identity. Historical RFC examples
may remain when they explicitly describe the superseded surface.

## Motivation

RFCs 0236 through 0239 intentionally kept the old model Functions and
decorators while syntax, concrete identity, recursive sealing, and family
applications were implemented independently. Keeping both surfaces after that
transition would leave two materially different meanings for visually similar
declarations:

- keyword initializers create declaration-owned identity;
- decorator declarations and callable model constructors produce forgeable
  structural metadata.

That ambiguity weakens the nominal guarantees and makes documentation,
diagnostics, inference tests, and user-space metadata interpreters describe
different languages. The transition ends atomically in this RFC.

## Public surface

`struct` and `enum` are contextual declaration-initializer keywords. They are
accepted only immediately after the `=` of a `type` binding and only in their
defined braced forms. They are not ordinary identifiers in expression or
import position.

```telora
type Record = struct {value: Int};
type Choice = enum {'Empty, 'Value(Int)};

let x = struct('None, {value: Int}); # unknown/reserved binding error
import "std/prelude" {struct};       # no such export
```

The implementation should diagnose a removed declaration decorator directly
when practical. It need not parse or execute old programs for compatibility.

`struct` and `enum` remain available as ordinary domain binding names only
where the lexer can unambiguously treat them as identifiers outside the
contextual initializer position. No standard binding with either name exists,
and source code cannot use those spellings to invoke the private normalizer.

## Decorators

Ordinary root, field, and variant decorators remain supported:

```telora
@json.rename_all('CamelCase)
type Event = enum {
    'Idle,
    @json.rename("failed") 'Failed(String),
};

type Request = struct {
    @json.rename("requestId") id: String,
};
```

Root decorators transform the structural draft before its declared wrapper is
sealed. They must preserve the initializer root kind and cannot replace,
recover, or mint declaration identity. Field and variant decorators retain
their established contexts and ordering.

`@struct` and `@enum` are not special-cased aliases for keyword initializers.
If no user binding with those names exists, they fail as unknown decorators.

## Dynamic metadata

This removal does not make TypeMetadata syntax-only. Telora code may still:

- pass declared TypeMetadata and TypeMetadata families as values;
- construct canonical anonymous metadata through the remaining public
  TypeMetadata constructors such as `Array`, `Dict`, `Tuple`, and `Func`;
- compute or transform canonical metadata Dicts where existing APIs accept
  them; and
- apply ordinary decorators and metadata interpreters.

There is no public Function that converts an arbitrary field or variant map
into a declared Struct/Enum identity. Active code that used callable
`struct`/`enum` for genuinely dynamic anonymous metadata must either use its
canonical data representation deliberately or be redesigned around a direct
declared initializer. It must not be mechanically rebranded as a declaration.

## Nominal migration rules

Repository migration observes the completed RFC 0237 semantics:

1. direct keyword Struct/Enum initializers mint identity;
2. aliases and reexports retain identity;
3. displays use authored declared names rather than expanded bodies;
4. expected literals and variants acquire their exact owner at the
   construction site;
5. external data may acquire identity only through an authorized typed
   boundary such as codec decode or witness-directed validation;
6. raw computed records do not become declared values by late structural
   comparison; and
7. tests that intentionally need anonymous structural metadata must say so
   rather than rely on the removed declaration decorator.

Old Enum descriptor-map source is rewritten to tagged declaration syntax:

```telora
@enum type ResultValue(Value, Error) = {
    Ok: Value,
    Err: Error,
};
```

becomes:

```telora
type ResultValue(Value, Error) = enum {
    'Ok(Value),
    'Err(Error),
};
```

## Prelude and implementation boundary

The public prelude, its static interface, native registration table, source
module, and tests expose no `struct` or `enum` Function. Open imports and
selective imports cannot discover either name.

Contextual initializer lowering may continue to emit a private core-model
operation. That operation:

- has no ModuleId-visible export;
- is constructible only by trusted compiler lowering;
- remains quota-accounted and source-located;
- normalizes the same canonical Struct/Enum body used by declared sealing; and
- cannot be reached through `Any`, reflection, import aliasing, or a forged
  String/Atom name.

Removing public constructors must not remove public TypeMetadata observation.
TypeDesc, Dyn, codecs, schema, equality, debug/show, LSP, and Host adaptation
continue to observe declared bodies and names according to RFCs 0237–0239.

## Repository migration

The implementation migrates:

- core and standard Telora modules;
- active Rust parser, type, VM, module, codec, schema, recovery, and CLI
  fixtures;
- active `.telora` source fixtures and workspace examples;
- `docs/design/LANGUAGE.md` as the language SSOT;
- current language and CLI tutorials;
- current experiment plans and injected language tutorials; and
- user-space interpreters and metadata helpers that depended on expanded
  structural display.

Archived experiment outputs and historical RFCs are not silently rewritten.
They may retain old source as historical evidence, but current instructions
must not teach it as accepted syntax.

The migration also updates expected type displays from expanded shapes to
declared names and supplies explicit construction context where an old test
relied on late structural widening.

## Diagnostics and recovery

Malformed keyword initializers retain RFC 0236 CST recovery. Removed forms
produce deterministic syntax, unknown-binding, unknown-decorator, or missing
export diagnostics as appropriate. No fallback retries an old parse or invokes
a compatibility normalizer.

Recovery and `show` may retain partial structural facts from a damaged
initializer, but they must not publish a completed declared identity until the
initializer can be sealed.

## Acceptance criteria

This RFC is complete when:

1. no public prelude/core interface exports callable `struct` or `enum`;
2. `@struct` and `@enum` are absent from current language surfaces;
3. explicit source calls to the removed constructors fail deterministically;
4. all active declarations use keyword Struct/Enum initializers and tagged
   Enum members;
5. decorators retain order, context, attributes, provenance, and quota
   accounting on declared roots and members;
6. standard modules, codecs, schema, TypeDesc, Dyn, debug/show, LSP, Host
   boundaries, and recovery pass with nominal identities;
7. current SSOT, tutorials, and experiment inputs teach only the final model;
8. no compatibility alias, parser retry, structural identity recovery, or
   silent `Any` approximation remains;
9. repository searches find removed syntax only in historical material or
   explicit negative tests; and
10. workspace formatting, tests, and linting pass.

## Implementation plan

1. remove prelude and module-interface exports for callable model constructors;
2. make initializer lowering target private core-model operations directly;
3. add focused negative tests for calls, imports, and legacy decorators;
4. migrate active Rust and Telora fixtures, including tagged Enum syntax and
   nominal display assertions;
5. adapt external data validation and user-space metadata interpreters to
   declared witnesses without shape-based identity recovery;
6. migrate standard modules, SSOT, tutorials, and current experiments;
7. remove transitional compatibility language from RFC 0236 and RFC 0237;
8. audit repository searches, full workspace tests, clippy, and resource
   accounting; and
9. mark RFC 0235 and RFC 0240 implemented only after the complete repository
   is green.

## Non-goals

This RFC does not add:

- parameterized recursive families;
- positional Structs, tuples with nominal identity, or newtypes;
- public declaration identity reflection or casts;
- a second type-level evaluator or kind system;
- compatibility with legacy declarations; or
- migration of immutable historical experiment results.
