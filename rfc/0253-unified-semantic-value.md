# RFC 0253: Unified semantic `Value` for data formats

- Status: Implemented
- Scope: JSON, YAML, TOML, static data modules, and codec boundaries
- Tracking: #100, #101

## Summary

Telora defines one public, nominal, recursively tagged `std/value.Value` for
interchange data. JSON, YAML, and TOML parsers normalize their input to this
type. Static data files are modules with a single `data: Value` export. Typed
models cross this boundary through `std/codec`; `Any` and `cast!` are not data
decoders.

```telora
type Value = enum {
    'None,
    'True,
    'False,
    'Int(Int),
    'Float(Float),
    'String(String),
    'Bytes(Bytes),
    'Array(Array(Value)),
    'Object(Dict(Value)),
    'LocalDate(String),
    'LocalTime(String),
    'LocalDateTime(String),
    'OffsetDateTime(String),
};
```

`Value` is a semantic sum type, not a lossless syntax tree. It deliberately
does not preserve comments, anchors, scalar spelling, table spelling, or VM
layout.

## Motivation

The current data frontends publish raw VM arrays, dictionaries, atoms, and
scalars behind `Any`. That makes a temporary dynamic-kernel representation a
public language contract. It prevents exhaustive matching, obscures the
format boundary, and lets static imports and runtime parsing expose different
module shapes.

A common tagged type makes every accepted input explicit and gives recursive
containers one canonical nominal identity. The VM may specialize this type in
the future without changing source semantics.

## Standard modules

`std/value` owns and exports `Value`. It is installed before every format and
codec module, and all of those modules import the same declaration. A second
structurally identical declaration is not a `Value`.

The public parsing contracts are:

```telora
// std/json
parse: Fn(String) -> Result(Value, BlameError);

// std/yaml
parse: Fn(String) -> Result(Value, BlameError);

// std/toml
parse: Fn(String) -> Result(Value, BlameError);
```

`BlameError` remains the current native structured diagnostic carrier. In
these APIs it has the role described as `DecodeError` by the surface design;
introducing a second native error representation is outside this RFC.

Raw JSON encoding accepts `Value`. Typed values are first lowered through
`codec.encode(Value, model)`. Existing codec attributes continue to describe
that model lowering and are not moved into the format parser.

## Static data modules

A `.json`, `.yaml`, `.yml`, or `.toml` file has a semantic module interface,
not a raw root value:

```telora
import "./data.json" { data as json_data };
import "./data.yaml" { data as yaml_data };
import "./data.toml" { data as toml_data };
```

Each imported binding has type `Value`. Namespace import exposes `module.data`.
The module root is an exports object and the interface declares both the
`data` export and the imported canonical `Value` type identity. The same
artifact shape is used by strict loading, recovery, `check`, and `show`.

The format frontends may continue to build a private raw graph while parsing.
Before publication, a single lowering recursively wraps every node in the
corresponding `Value` variant and preserves each node's source location.
Provenance paths remain expressed in terms of semantic array indices and
object keys; the implementation-only variant wrapper does not add path
segments.

## Deterministic format mappings

### JSON

- null, booleans, strings, integers, and finite floats map to their scalar
  variants;
- arrays and objects recursively map to `Array` and `Object`;
- integer overflow and non-finite floating-point values fail at the source
  token and are never stringified or clamped.

### TOML

- booleans, strings, integers, finite floats, arrays, and tables use the common
  variants;
- local date, local time, local date-time, and offset date-time use their
  dedicated variants with validated normalized String payloads;
- ordinary and inline tables both normalize to `Object`;
- integer overflow and non-finite floats fail.

### YAML

Telora accepts a fixed deterministic schema rather than YAML's complete object
model:

- null, booleans, strings, integers, finite floats, sequences, and string-key
  mappings use the common variants;
- the standard binary scalar maps to `Bytes` after strict base64 validation;
- aliases are expanded with bounded depth and total work; cycles fail;
- merge keys expand mapping aliases in source order, while explicit fields
  deterministically override merged fields and duplicate effective keys fail;
- non-string mapping keys and unknown/custom tags fail;
- anchors, aliases, tags, comments, and original scalar spelling are not
  retained.

## Codec and cast boundaries

`std/codec` is the only standard graph conversion boundary:

```telora
codec.decode(User, value)       // Result(User, BlameError)
codec.encode(Value, user)       // Result(Value, BlameError)
```

Decode recursively removes `Value` variants before applying the target model
schema. Encode applies model attributes first and recursively constructs a
`Value`. Nested path diagnostics, rename, rename_all, default, flatten,
untagged, and skip rules keep their existing meaning.

`ty!` proves a static fact without changing a value. `cast!` performs a
representation-preserving checked refinement. Neither operation parses,
unwraps, or reconstructs `Value`; `Value -> User` therefore cannot be
implemented as a cast.

`Any` remains an internal dynamic-kernel erasure and compatibility tool. No
public raw parse or static data interface introduced here exposes it.

## Rejected alternatives

- A recursive untagged union would couple surface semantics to VM `meta` and
  require transparent recursive structural canonicalization.
- Keeping raw static roots while changing only runtime signatures would leave
  two incompatible format contracts.
- Attaching only a `Value` witness to the raw root would not make nested values
  exhaustive tagged values.
- Decoding through `cast!` would violate its representation-preserving
  contract and bypass codec attributes.
- Separate JSON, YAML, and TOML value ADTs would force needless conversion and
  prevent format-independent enterprise knowledge.

## Implementation plan

1. Add and install `std/value` before codec and format modules.
2. Add one provenance-preserving raw-graph-to-`Value` materializer and its
   inverse view for codec/encoding.
3. Publish static data as a typed `{ data }` module in both strict and recovery
   loaders.
4. Change JSON parsing and encoding to `Value`; add equivalent YAML and TOML
   runtime parse modules.
5. Adapt codec decode/encode at the public boundary while retaining its
   schema-directed internal transformer.
6. Add mapping, rejection, identity, exhaustive-match, provenance, and codec
   decorator regressions; update SSOT and tutorials.

## Acceptance criteria

1. No JSON/YAML/TOML raw parse or static import publicly returns `Any`.
2. Every scalar, recursive container, and TOML temporal node has the canonical
   `Value` TypeId and can be exhaustively matched.
3. Static files export exactly `data: Value` in strict and recovery views.
4. JSON rejects overflow/non-finite numbers; TOML rejects overflow/non-finite
   numbers; YAML rejects non-string keys, cycles/limits, and custom tags while
   deterministically expanding aliases and merge keys.
5. `codec.decode` and `codec.encode(Value, ...)` preserve nested diagnostics
   and all existing codec decorators.
6. Source provenance survives normalization without wrapper-only path
   segments.
7. Language SSOT, tutorials, and standard module documentation describe the
   semantic model and its boundaries.
8. The complete workspace test suite passes.
