# RFC 0153: Regex string-parse provider

- Status: Implemented
- Superseded by [RFC 0303](0303-static-from-str-and-regex-parsing.md): regex publishes a type property used by a static `FromStr` fallback implementation.
- Parent: RFC 0151
- Depends on: RFC 0149, RFC 0150, RFC 0152

## Summary

`std/regex` installs regex parsing as a `std/string.parse` capability:

```forma
@re.parse_by(re.compile(r"(?P<name>\w+)=(?P<value>\d+)"))
@struct
type Rec = { name: String, value: Int };

string.parse(Rec, "answer=42")
```

`re.parse_by` replaces `re.parse`. The provider-neutral `string.parse`
replaces `re.decode`; regex keeps `compile` and `is_match`.

## Validation and execution

At decoration time, `parse_by` verifies that:

- the decorated type is a struct;
- every capture is named;
- capture names and struct field names are identical;
- required captures map to required fields;
- optional captures map to `Option(T)` fields;
- every required field type, or optional payload type, has a known string
  parser.

At parse time, the provider matches the complete source, obtains each named
capture, and recursively invokes the shared string parser for its field type.
It adds only the `Option` wrapper required by capture participation.

## Migration

RFCs 0148 through 0150 record the first regex-specific API. This RFC amends
that public surface without rewriting those historical decisions:

```text
re.parse(regex)       -> re.parse_by(regex)
re.decode(ty, source) -> string.parse(ty, source)
```

## Acceptance criteria

1. Regex owns matching and capture extraction, not scalar conversion.
2. Field conversion delegates to the shared recursive parse dispatcher,
   including provider-backed named struct fields reached through type links.
3. `Option(T)` capture semantics distinguish absence from a failed parse.
4. The old regex-specific decode entry point is no longer exported.
5. Match and field-conversion failures return `BlameError` from
   `string.parse`.

## Implementation result

Implemented in August 2026. Regex metadata now implements the shared string
parse protocol, named nested struct providers follow ordinary TypeDesc links,
and regex no longer owns scalar conversion.
