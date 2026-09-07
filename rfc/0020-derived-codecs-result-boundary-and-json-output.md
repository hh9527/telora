# RFC 0020: Derived Codecs, Result Boundary, and JSON Output

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Decoding returns DecodeError with its current Value; encoding returns Value directly.

## Summary

XL gains the smallest end-to-end data application path that separates external
representation, normalized domain data, and output serialization:

```xl
import data from "./abc.json";
import User from "./User.xl";
import result from "core:result";
import json from "core:json";

let user = data |> User.decode |> result.unwrap;
let text = user |> User.encode |> json.stringify_pretty(2);
text
```

`User.xl` is an ordinary XL module. It defines Type metadata and builds its
boundary functions from `core:codec`:

```xl
import codec from "core:codec";
import result from "core:result";

fn Optional(item) {
    Union([Atom('None), Tuple([Atom('Some), item])])
}

type Type = Struct({v: Optional(String)});

let decode = fn(value) { codec.decode(Type, value) };
let encode = fn(value) { codec.encode(Type, value) |> result.unwrap };

{Type, decode, encode}
```

The codec is derived from ordinary Type metadata. No nominal Struct runtime
value, hidden field attribute, or schema-specific VM instruction is added.

## Representation boundary

Three value domains are kept explicit:

```text
external JSON-shaped value
    -- codec.decode(Type, value) --> Result(normalized XL value, error)
normalized XL value
    -- codec.encode(Type, value) --> Result(JSON-shaped value, error)
JSON-shaped value
    -- json.stringify* --> String
```

Decode and encode are not validation aliases. They may recursively rebuild a
value. Validation remains useful when no representation change is wanted.

The first codec supports existing Type metadata kinds: Any, Int, Float,
String, Array, Tuple, Struct, and Union. Bytes and Function are rejected at a
JSON codec boundary. Atom types support the built-in JSON atoms `'None`,
`'True`, and `'False`; other atoms have no implicit JSON representation.

## Option convention

No new Option type kind is introduced. A Union with exactly these variants is
recognized as the standard Option convention:

```text
Union([Atom('None), Tuple([Atom('Some), item])])
```

For an Option-valued Struct field, decoding follows these rules:

| external field | normalized field |
| --- | --- |
| missing | `'None` |
| JSON null (`'None`) | `'None` |
| present non-null `v` | `('Some, decode(item, v))` |

Thus all three inputs below decode successfully:

```text
{"v":"abc"} -> {v: ('Some, "abc")}
{"v":null}  -> {v: 'None}
{}            -> {v: 'None}
```

Encoding is canonical rather than presence-preserving. `'None` emits a present
field containing JSON null, and `('Some, value)` emits the encoded item. The
codec does not remember whether an input field was absent or explicitly null.

Non-Option Struct fields are required. Unknown external and normalized fields
are rejected in this RFC. Defaults, rename rules, aliases, flattening, open
Structs, and attribute syntax are deferred.

## Results

`core:codec.decode` and `core:codec.encode` return ordinary tagged tuples:

```text
('Ok, value)
('Err, message)
```

The initial error payload is a deterministic String containing a `$`-rooted
value path and the failed expectation. Nested paths use `.field` and `[index]`.
The representation is intentionally narrow; structured multi-diagnostics and
runtime provenance attachment are deferred without changing the Result tag.

`core:result` initially exports `unwrap(result)`. It returns the payload of
`('Ok, payload)`. For `('Err, message)` it raises a runtime error containing
the message. Any other input is a runtime type error. `unwrap` is an explicit
application boundary, not syntax and not an implicit VM propagation rule.

## JSON output

`core:json` exports:

```text
stringify(value)
stringify_pretty(indent) -> fn(value) -> String
```

`stringify_pretty` validates `indent` as an Int in `0..=16` and returns a
native closure that captures it. This deliberately exercises the same
prototype-plus-upvalues representation as XL closures. Pipeline behavior stays
uniform:

```xl
value |> json.stringify_pretty(2)
```

is exactly:

```xl
json.stringify_pretty(2)(value)
```

JSON serialization accepts Int, finite Float, String, Array, Dict, and the
built-in atoms `'None`, `'True`, and `'False`. It rejects Bytes, Tuple, Func,
up-links, non-JSON atoms, non-finite Floats, and internal cycles. Dict fields
are emitted in canonical shape order. Compact output has no insignificant
whitespace; pretty output uses LF and the configured number of spaces per
level. String escaping follows JSON, including control characters.

Unlike debug formatting, JSON output is an XL String and is charged to the
current execution allocation quota. Serialization traverses runtime heap views
directly and does not deep-export through legacy `Value`.

## Static analysis

Core module shapes and function arities remain statically visible. Generic
relationships may initially degrade to Any:

```text
codec.decode(Type, Any) -> Result
codec.encode(Type, Any) -> Result
result.unwrap(Result) -> Any
```

`User.decode` and `User.encode` are ordinary closures. Precise propagation of
the captured Type parameter is deferred; runtime codec behavior remains exact.

## Diagnostics and provenance

Codec failures retain a deterministic logical path. Imported JSON provenance
currently lives in a module-loader side table rather than in runtime values, so
the initial runtime codec cannot attach the original JSON span to its Result
payload. The rule-side call still receives ordinary debug origins.

Connecting runtime paths back to loader provenance, and returning structured
diagnostic records with data and rule labels, is required follow-up work. This
RFC does not move spans into runtime values or silently fabricate locations.

## Fuel and quotas

Each core call consumes ordinary call fuel. Codec and JSON traversal are native
work proportional to reachable input size; the implementation charges
allocation for produced XL graphs and Strings. The current control-flow fuel
model does not price every visited element, consistent with Array and Dict core
operations. Existing depth, stack, allocation, and input limits remain the
resource boundary.

## Rejected alternatives

### Treat decode as validation

Validation cannot turn a missing or null field into a normalized Option value.
Keeping the operations distinct makes representation changes visible.

### Add serde-style field syntax first

Attributes are surface sugar over codec metadata. Implementing the executable
protocol first establishes semantics that future `#[decode(...)]` syntax can
lower into.

### Serialize arbitrary XL values

Functions, Bytes, tagged tuples, and named atoms have no unique JSON meaning.
Implicit conventions would hide boundary errors. `User.encode` is the explicit
place to remove domain representations such as `Some` tuples.

### Make stringify_pretty a special pipeline case

It is an ordinary curried native closure. The pipeline remains reverse
application with no argument insertion rule.

## Deferred work

- serde-style field attributes and Struct declaration syntax;
- rename, alias, default, flatten, open/deny-unknown, and coercion policies;
- structured codec diagnostics and runtime provenance lookup;
- YAML, TOML, JSONL, and text encoders;
- streaming encoders and output writers;
- precise generic static types for codecs and Result;
- Result combinators and propagation syntax;
- user-defined codec protocols and recursive Type metadata;
- configurable JSON key order or formatting beyond indentation.

## Implementation plan

1. Add `core:codec`, `core:result`, and `core:json` module identities.
2. Implement heap-view Type decoding and recursive decode/encode transforms.
3. Recognize the standard Option metadata shape at Struct field boundaries.
4. Implement `result.unwrap` as an explicit runtime-error boundary.
5. Implement strict compact JSON serialization and curried pretty closures.
6. Charge produced runtime values and Strings through the existing quota path.
7. Add a `User.xl` scenario and tests for present, null, missing, and invalid
   fields, canonical round trips, pretty output, diagnostics, and quotas.

## Acceptance criteria

1. The Summary program loads and produces deterministic pretty JSON.
2. Present String, explicit null, and missing Option fields normalize to the
   specified values.
3. Encode reverses normalized Option representation and emits canonical null.
4. Required, unknown, and incorrectly typed fields return path-bearing Err.
5. `result.unwrap` returns Ok payloads and raises Err payloads as runtime errors.
6. Compact and pretty JSON are strict, deterministic, and reject non-JSON
   runtime categories and cycles.
7. `stringify_pretty(2)` returns an ordinary captured unary native closure and
   works through unchanged pipeline semantics.
8. Codec and stringify traverse heap views without legacy deep export and
   charge all produced XL allocations.
9. Existing library APIs, module initialization, session quotas, and debug
   observation remain compatible.

## Implementation result

Implemented with three ordinary reserved core modules and VM-managed native
functions. `core:codec` decodes Type metadata directly through `HeapView` into
an internal output plan, validates the complete transform, charges its logical
allocation, and only then materializes the result in the local heap. No legacy
`Value` export is used. Struct Option fields implement the specified
present/null/missing normalization and encode canonically back to null or the
contained JSON-domain value.

`core:result.unwrap` provides the explicit fatal boundary. `core:json` contains
strict compact and pretty writers that traverse heap handles directly, escape
JSON strings, reject cycles and XL-only categories, preserve canonical Dict
order, and charge the resulting XL String. `stringify_pretty` returns a native
closure whose single Int upvalue stores the indentation width.

The executable `examples/codec` directory contains the Summary flow. Module
and VM tests cover all three Option inputs, invalid paths, required and unknown
fields, compact and pretty output, escaping, invalid indentation, non-JSON
Tuple rejection, internal cycles, and allocation quota failure.

Implementation also corrected an existing canonical-order bug in tool-stage
Union metadata validation: Union fields are `kind, variants`, not `variants,
kind`. The bug affected the earlier MVP Optional example and was exposed by the
new schema module.

As specified, runtime codec errors currently carry logical paths but not JSON
source spans. Connecting those paths to loader provenance remains deferred.
