# RFC 0152: String parse capability

- Status: Implemented
- Superseded by [RFC 0303](0303-static-from-str-and-regex-parsing.md): the public entry is `string.parse@[T](text)` with static `T: FromStr` evidence.
- Parent: RFC 0151
- Depends on: RFC 0056, RFC 0123

## Summary

`std/string` exports:

```forma
native parse: for(A) Fn(TypeOf(A), String) -> Result(A, BlameError);
```

The initial built-in cases are identity for `String`, decimal conversion for
`Int`, and floating-point conversion for `Float`. A parse failure has `rule`
set to the supplied type metadata and `data` set to the source string.

## Provider protocol

An attributed type may contain the private protocol key
`std/string.parse`. Its value identifies a provider and contains the
provider-specific immutable data. The initial non-built-in provider is
`'Regex(regex)` as installed by RFC 0153.

Provider lookup follows nested `WithAttributes` wrappers. The protocol is an
implementation contract between standard-library modules, not a user-facing
generic registry. Unknown or malformed provider metadata is rejected.

## Acceptance criteria

1. `string.parse(String, source)` returns `'Ok(source)`.
2. `string.parse(Int, source)` accepts a complete decimal integer and reports
   invalid or overflowing input as `BlameError`.
3. A type without a built-in parser or valid provider returns `BlameError`.
4. The result remains statically tied to the supplied type object.

## Implementation result

Implemented in August 2026. Built-in and provider-backed parsing share one
native recursive dispatcher while preserving the public polymorphic type.
