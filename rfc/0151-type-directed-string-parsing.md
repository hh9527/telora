# RFC 0151: Type-directed string parsing

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Parsing failures return ParseError with the input String.
- Depends on: RFC 0056, RFC 0123, RFC 0148
- Child RFCs: RFC 0152 through RFC 0153

## Summary

Forma provides one type-directed entry point for constructing values from
text:

```forma
import "std/string" as string;

string.parse(Int, "42")
```

`string.parse` has the signature:

```forma
for(A) Fn(TypeOf(A), String) -> Result(A, BlameError)
```

It is Forma's traitless counterpart to the family of capabilities commonly
called `FromStr`. The type object is the authority for the result type and may
carry metadata selecting a parsing provider.

## Direction

The call site does not select an implementation module. Built-in types are
handled directly; decorated types retain provider metadata in their
`TypeDesc`; and `string.parse` dispatches through that metadata. This keeps a
stable typed entry point while allowing the set of providers to grow.

The initial providers are deliberately narrow:

1. built-in parsing for `String`, `Int`, and `Float`;
2. regex-backed struct parsing installed by `std/regex.parse_by`.

A later RFC may allow a type to install an ordinary Forma parser function.
This RFC does not introduce a general capability registry, traits, dynamic
code generation, or module synthesis.

## Regex integration

```forma
import "std/regex" as re;
import "std/string" as string;

@re.parse_by(re.compile(r"(?P<name>\w+)=(?P<value>\d+)"))
@struct
type Rec = {
    name: String,
    value: Int,
};

string.parse(Rec, "answer=42")
```

The regex provider only matches and splits the input. Each present capture is
decoded by recursively applying `string.parse` to the field type. Regex does
not own a fixed list of scalar conversions.

For a field of type `Option(T)`, optionality describes capture participation:

- an absent capture produces `'None`;
- a present capture is parsed as `T` and wrapped in `'Some`.

This rule does not imply a general top-level parser for `Option(T)`.

## Child sequence

RFC 0152 defines `std/string.parse`, its typed contract, built-in parsing, and
the provider metadata protocol.

RFC 0153 migrates regex parsing from `re.parse` plus `re.decode` to
`re.parse_by` plus `string.parse`, and makes capture conversion recursive.

## Acceptance criteria

1. `string.parse` returns `Result(A, BlameError)` for a supplied `TypeOf(A)`.
2. `String`, `Int`, and `Float` parse without attached metadata.
3. Provider metadata remains attached to the type object and is preserved
   through nested attributed wrappers.
4. Regex-backed fields use the same parsing protocol recursively.
5. Unsupported types fail as values with `BlameError`, not by weakening the
   result type.
6. The previous RFCs remain historical records; this sequence documents the
   public migration.

## Implementation result

Implemented in August 2026 through RFCs 0152 and 0153. `std/string.parse` is
the sole public typed text-decoding entry point, with built-in and regex
providers sharing one recursive implementation.
