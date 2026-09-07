# RFC 0056: Typed blame errors and Result composition

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): BlameError is replaced by producer-specific error contracts and privileged Diagnostic snapshots.
- Depends on: RFC 0020, RFC 0021, RFC 0031, RFC 0032, RFC 0036, RFC 0052, RFC 0053, RFC 0055

## Amendment

The original accepted text exposed separate `DecodeError`, `EncodeError`, and
`ValidationError` names with identical structural definitions. Its first
implementation then used `ValidationError` in all three native contracts while
still exporting codec-owned aliases, exposing that the names did not represent
an observable semantic distinction.

This amendment replaces all three names with `BlameError`. The change is a
design convergence, not merely an implementation workaround: codec,
validation, JSON parsing, and future data-loading boundaries all report the
same data-versus-rule relationship. Environmental I/O failures remain outside
this type.

## Summary

XL gives codec and validation failures public structural types and completes
the small Result surface needed to compose typed external-data boundaries:

```xl
import codec from "core:codec";
import result from "core:result";

@struct type User = {name: String};

codec.decode(User, input)
|> result.map(fn(user) { user.name })
|> result.map_err(codec.format_error)
```

The boundary contracts become:

```xl
codec.decode:
    for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError)
codec.encode:
    for(A) Fn(TypeOf(A), A) -> Result(Any, BlameError)
validate:
    for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError)
```

All three operations use one public boundary-diagnostic type:

```xl
@struct type BlameError = {
    message: String,
    data: Any,
    rule: Any,
};
```

`BlameError` means that a data value does not satisfy a rule. It is deliberately
shared across phases and APIs: the relationship between the two blamed values
is more fundamental than whether codec, validation, or module loading observed
the mismatch.

## Motivation

RFC 0055 preserves the successful instance type through `TypeOf(A)`, but the
codec error parameter remains `Any` and validation still returns `String`.
That loses information at the exact boundary where applications need to
format, inspect, transform, or propagate failures.

The runtime codec already returns a deterministic Dict containing `message`,
the rejected `data`, and the responsible `rule`, with source locations carried
by rich values. This RFC exposes that existing representation rather than
inventing a second diagnostic model. Validation adopts the same representation
so callers can handle both boundaries uniformly.

Typed boundaries also remove the reason RFC 0053 kept `result.unwrap` dynamic.
Its runtime behavior already preserves the Ok payload, so its declaration can
now express that relationship directly.

## Blame error model

The prelude exports one TypeMetadata value:

```xl
@struct type BlameError = {message: String, data: Any, rule: Any};
```

`core:codec` uses that shared type rather than defining codec-owned aliases:

```xl

native decode:
    for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError);
native encode:
    for(A) Fn(TypeOf(A), A) -> Result(Any, BlameError);

def format_error:
    Fn(BlameError) -> String
= fn(error) { error.message };
```

Validation uses the same contract:

```xl
native validate:
    for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError);
```

On validation failure, `data` is the rejected value and `rule` is the supplied
TypeMetadata value. Success still returns the original input value without
copying or normalization.

`format_error` returns the deterministic message already produced by the
runtime. It does not discard locations from the original error value; it merely
projects a String for display-oriented consumers.

The initial `message` remains the existing deterministic logical-path String.
For example, `value.user.name must be String` remains a String rather than
being parsed into path segments. A typed path algebra should be based on real
consumer requirements and is deferred.

## Result composition

`core:result` refines and extends its ordinary generic API:

```xl
native unwrap: for(A, E) Fn(Result(A, E)) -> A;

def flat_map:
    for(A, E, B) Fn(Result(A, E), Fn(A) -> Result(B, E)) -> Result(B, E);
```

`flat_map` calls its callback only for `Ok` and preserves `Err` unchanged. It
uses the same name as `core:option.flat_map`; no method or trait dispatch is
introduced. Existing `map`, `map_err`, `unwrap_or`, and `is_ok` contracts are
unchanged.

`unwrap` remains an explicit runtime-failure boundary. A structured error uses
its `message`, `data`, and `rule` fields to retain the existing dual-source
diagnostic. Only its static contract changes: a known `Result(A, E)` now
returns `A`, while `Any` remains accepted through XL's gradual boundary and is
checked at runtime.

## Runtime and provenance semantics

Codec failure representation is unchanged. Validation changes its Err payload
from a String to a Dict with the public error shape. The Dict and its fields
retain compact rich-value locations:

- `data` refers to the rejected value and carries its data-side location;
- `rule` refers to the TypeMetadata argument and carries its rule-side location;
- `message` is located at the rejected value when such a location exists.

`result.unwrap` continues to recognize both legacy String errors and public
structured errors at runtime. This preserves compatibility for user-created
Results and previously compiled modules.

No exception hierarchy, stack unwinding rule, VM opcode, heap object kind, or
native ABI is added.

## Files and phase boundaries

An imported JSON document is also data checked against rules. A malformed JSON
file blames source bytes against the JSON grammar, while a decoded document
blames a JSON-shaped value against TypeMetadata. The current module loader may
turn either failure directly into a compile-time diagnostic, but this phase
choice does not require a separate error model.

A future ordinary file capability can expose the stages explicitly:

```xl
file.read(path)       # Result(Bytes, IoError)
json.parse(bytes)     # Result(Any, BlameError)
codec.decode(T, data) # Result(T, BlameError)
```

Missing files, permissions, and transport failures are `IoError`, not
`BlameError`, because no data-versus-rule mismatch occurred. A convenience
loader may combine them with an ordinary Tagged error type. Future replacement
of data imports with functions can therefore reuse `BlameError` without
conflating environmental failure with invalid content.

## Static semantics and module boundaries

`BlameError` has type `TypeOf({message: String, data: Any, rule: Any})`. Its use
in native declarations is trusted exactly like every other declarative native
capability.

The concrete structural error descriptors cross `ModuleInterface` and
workspace snapshots through the existing type graph. Member observation must
show typed error parameters rather than `Any` or `String`.

`format_error` and `flat_map` are ordinary XL definitions compiled with their
explicit generic contracts. Their implementation must not require privileged
Rust callbacks.

## Non-goals

- interface, trait, associated-type, or effect systems;
- a general language-wide `Error` supertype;
- parsing message Strings back into structured paths;
- accumulating multiple failures or defining applicative validation;
- treating host filesystem or network errors as blame;
- changing codec matching, normalization, or JSON representation rules.

## Implementation plan

1. add `BlameError` metadata to the runtime and static preludes;
2. refine codec native declarations and add ordinary `format_error`;
3. use `BlameError` in the validation contract;
4. return the structured payload from validation failures;
5. refine `result.unwrap` and add ordinary `result.flat_map`;
6. preserve legacy unwrap handling for String Err payloads;
7. publish all new types through module and workspace interfaces;
8. add runtime, static-observation, provenance, and composition tests.

## Acceptance criteria

1. `BlameError` is observable as a precise `TypeOf` witness;
2. `decode(User, input)` has type `Result(User, BlameError)`;
3. `encode(User, user)` has type `Result(Any, BlameError)`;
4. `validate(User, input)` has type `Result(User, BlameError)`;
5. all three failures contain `message`, `data`, and `rule` with stable runtime
   values and useful source locations;
6. `codec.format_error` accepts codec and validation errors by structural
   compatibility and returns their message;
7. `result.flat_map` preserves the error type and transforms the Ok type;
8. `result.unwrap` returns the statically known Ok type;
9. legacy String Err values still unwrap into runtime failures;
10. workspace tests, formatting, clippy, and strict static checks pass.

## Deferred work

- a structured `ValuePath` with field, index, and variant segments;
- diagnostic accumulation and `validate_all`-style APIs;
- stable error codes and machine-oriented expectation payloads;
- an ordinary file capability, `IoError`, and source-provider Results;
- nominal or capability-based error abstraction.

## Implementation result

The implementation exposes exactly one prelude metadata binding,
`BlameError`, and uses it in the native `decode`, `encode`, and `validate`
schemes and the ordinary XL `format_error` definition. The earlier
`codec.DecodeError`, `codec.EncodeError`, and prelude `ValidationError` names
were removed rather than retained as compatibility aliases; they had been
introduced only by the first implementation of this RFC and had no distinct
runtime representation.

Validation failures now use the same existing `{message, data, rule}` rich
value representation as codec failures. Result composition, legacy String Err
unwrapping, gradual `Any` calls, and source-location behavior remain as
implemented by the original RFC.

Workspace observation expands structural metadata names and therefore shows
the concrete `{data: Any, message: String, rule: Any}` error shape in Result
types, while direct observation of `BlameError` shows the corresponding
`TypeOf` witness. No file-loading behavior changed in this implementation; the
file and phase model above records the contract for a later capability RFC.

## Rejected alternatives

### Keep validation errors as String

This preserves a smaller value but throws away the rejected data and rule
locations precisely when validation is used as an application boundary. It
also forces codec and validation callers into different error pipelines.

### Separate DecodeError, EncodeError, and ValidationError

Three structurally identical aliases imply distinctions that the values cannot
observe and make shared pipelines noisier. `BlameError` names the common
data-versus-rule relationship without claiming to represent unrelated I/O,
cancellation, quota, or program errors.

### Add a structured path immediately

The runtime currently produces deterministic path Strings across Struct,
Enum, attributes, flattening, and recursion. Converting every failure site to
a new path value is a separate behavioral change and should be motivated by
concrete consumers rather than bundled into type exposure.
