# RFC 0033: JSON Schema generation from codec metadata

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Schema output is the semantic Value representation.
- Depends on: RFC 0031, RFC 0032

## Summary

`core:json.schema(Type)` derives a JSON-domain XL value from the same normalized
metadata and attribute plans used by `core:codec.decode` and
`core:codec.encode`. The result can be inspected, transformed, or passed to
`json.stringify`; schema generation is data computation rather than a compiler
side channel.

```xl
import json from "core:json";

let document = json.schema(User);
document |> json.stringify_pretty(2)
```

## Contract

`schema` accepts one runtime TypeMetadata value and returns a JSON-compatible
Dict. Invalid metadata or JSON attribute configuration is a runtime type error
with the most specific available rule location.

The generated document describes values accepted by decode. Serialization-only
omission through `skip_serializing_if` therefore does not make a field optional
on input. A field is absent from `required` only when it has a default or is an
Option. Defaults are included as JSON Schema `default` only when their canonical
XL value can be encoded by the field codec.

## Mapping

- `Any` becomes an unconstrained schema `{}`.
- `Int`, `Float`, and `String` use JSON Schema primitive types.
- `Atom('None)` uses `{type: "null"}`; other Atom values use `const` Strings.
- canonical `Bool` and `Option(T)` Enum shapes retain their natural JSON
  boolean and `null | T` representations at every nesting level.
- `Array(T)` uses `{type: "array", items: schema(T)}`.
- Tuple uses a fixed `prefixItems` array and equal `minItems`/`maxItems`.
- Struct uses `type: "object"`, planned external `properties`, deterministic
  `required`, and `additionalProperties: false`.
- flattened Struct properties and requirements merge into the enclosing object;
  collisions are rejected by the shared plan.
- externally tagged Enum uses `oneOf` branches for unit String constants and
  single-property payload objects.
- untagged Enum uses `oneOf` over payload schemas.
- Union uses `anyOf` because overlap is permitted.
- Bytes and Function have no JSON schema and fail generation.

The root document includes the 2020-12 dialect URI under `$schema`. Nested
schemas do not repeat it. Definitions and `$ref` deduplication are deferred.

## Diagnostics and determinism

Schema generation reuses codec planning for rename, rename_all, flatten,
default, untagged, and collision validation. Property and variant order follows
canonical metadata order. Failures point at the same rich rules as codec
failures. Generated values are ordinary XL Dict/Array/String/Atom values and
are charged to the current execution allocation quota.

## Vertical validation

The acceptance fixture defines a model with nested flattening, renamed and
CamelCase fields, defaults, skip policies, and both tagged and untagged Enums.
It decodes representative JSON, encodes the canonical value, generates and
stringifies its schema, and verifies a malformed external value renders both
the JSON data location and the model rule location.

This is the architecture test: decorators produce ordinary metadata; codec and
schema consumers independently receive that data but share one semantic plan;
diagnostics remain rich values rather than generator-specific exceptions.

## Deferred work

- `$defs`, `$ref`, recursive metadata graphs, and stable definition names;
- descriptions, examples, numeric/string constraints, and deprecation markers;
- separate input and output schema documents;
- externally supplied schema dialects and OpenAPI projection;
- schema validation against an external JSON Schema implementation.

## Acceptance criteria

1. primitive, Array, Tuple, Struct, Enum, and Union mappings are generated.
2. Struct naming, flatten, required/default, and strictness match decode.
3. tagged and untagged Enum schemas match RFC 0032 representations.
4. generated schemas are ordinary JSON-compatible XL data and stringify.
5. unsupported runtime types and invalid attribute plans fail with rule origins.
6. generation obeys allocation quota accounting.
7. one vertical fixture exercises attributes, Struct, Enum, codec, diagnostics,
   schema generation, and JSON output together.

## Implementation plan

1. Add `json.schema` to the core module and native ABI.
2. Generate `CodecNode` schema trees from decoded CodecType and shared plans.
3. Materialize and charge the complete document at the native boundary.
4. Add focused mappings plus the vertical end-to-end fixture.

## Implementation result

Implemented as `json.schema(Type)`, an ordinary native function returning an XL
Dict. It decodes runtime TypeMetadata, reuses the Struct and Enum plans from the
codec, builds a metered `CodecNode` tree, adds the 2020-12 dialect URI, and
materializes the result into the current work world.

Focused fixtures cover Union, Array, Tuple, Bool, Option, stringification, and
allocation quota failure. The vertical fixture loads located JSON, decodes a
model combining nested flatten, CamelCase, default, skip, externally tagged
Enum, and untagged Enum, re-encodes it, generates and stringifies its schema,
then replaces the input with invalid JSON and verifies both data and rule
locations survive through `result.unwrap`.

No separate schema-side interpretation of JSON attributes was introduced.
Required fields, external names, flatten merging, defaults, strict object
boundaries, and Enum representations all come from the same plans used for
runtime transformation. This validates the intended metadata architecture for
the supported feature set rather than merely duplicating its surface syntax.
