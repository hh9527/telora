# RFC 0156: Container text-codec bridge

- Status: Implemented
- Superseded in its parse-selection mechanism by [RFC 0303](0303-static-from-str-and-regex-parsing.md): codec text bridges consume sealed `FromStr` evidence.
- Depends on: RFC 0056, RFC 0151, RFC 0154

## Summary

A type can select its canonical structured-codec representation as text:

```forma
@string.decode_by_parse
@string.encode_by_display
@re.parse_by(...)
@fmt.display_by("{host}:{port}")
@struct type Endpoint = { host: String, port: Int };
```

`codec.decode` accepts a String and delegates to the type's Parse capability;
`codec.encode` delegates to Display and produces a String. Nested containers
follow the same TypeDesc links and behavior.

## Rules

The attributes are container/type-level in this RFC. Field-level overrides are
deferred. `decode_by_parse` and `encode_by_display` must appear together so the
single codec and JSON Schema model remains symmetric. Missing partners,
malformed markers, or missing Parse/Display capabilities are contract errors.

JSON Schema projects the decorated type as `{ "type": "string" }`. Parse
failures retain the structured codec path and original String as blame data.
Parse and Display are not proven inverses; their round-trip relation remains a
contract owned by the type author.

## Acceptance criteria

1. Top-level and nested decorated containers decode from String.
2. Top-level and nested decorated containers encode to String.
3. JSON Schema reports String for the external representation.
4. The two bridge declarations are required as a pair.
5. No field-level policy is introduced.

## Implementation result

Implemented in August 2026 by sharing the compiled Parse and Display plans
with the codec transformer.
