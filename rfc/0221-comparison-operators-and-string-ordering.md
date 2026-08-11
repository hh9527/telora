# RFC 0221: Comparison Operators and String Ordering

- Status: Proposed
- Depends on: RFC 0003, RFC 0007, RFC 0010, RFC 0021, RFC 0052, RFC 0074,
  RFC 0075, RFC 0127
- Tracking issue: #19

## Summary

Complete Telora's conventional comparison surface:

```telora
left == right
left != right
left < right
left > right
left <= right
left >= right
```

Equality and inequality retain the existing structural equality domain.
Ordered comparison accepts matching `Int`, `Float`, or `String` operands.
String order is the deterministic lexicographic order of Telora's internal
UTF-8 byte sequence, with no locale, normalization, case-folding, or
natural-number behavior.

All six operators share one non-associative precedence level. They return the
canonical Bool values `'True` and `'False`. Operands evaluate exactly once in
source order.

## Motivation

Telora currently exposes only `==` and `<`. This is computationally close to a
complete comparison family for finite numeric values, but it is not a complete
authoring surface. Programs must reverse conditions or branches to express
ordinary inclusive bounds and inequality.

The opencode test-10 A2 experiment hit this directly: a depth guard was first
written with `>=`, failed to parse, and was rewritten as an inverted `<`
branch. The workaround is understandable but obscures the intended boundary
and makes generated code less direct.

Treating every missing operator as textual sugar is not semantically correct.
Telora Float arithmetic follows IEEE behavior and can produce NaN. For NaN,
`a <= b` is false while `!(b < a)` would be true. Inclusive comparisons must
therefore preserve primitive Float comparison behavior rather than being
specified as Boolean negation.

String ordering is also a general deterministic data operation. Configuration,
catalog, identifier, and generated-output logic often needs stable lexical
bounds. Putting locale-aware collation in a language operator would make the
same program environment-dependent, so this RFC defines only an exact ordinal
order. Richer human-language collation remains a library concern.

## Goals

1. provide the conventional six comparison operators;
2. preserve existing equality behavior for all currently comparable values;
3. support ordered comparison for matching Int, Float, and String values;
4. define Float NaN behavior without algebraically invalid rewrites;
5. define deterministic internal-UTF-8 String order independent of locale and
   platform;
6. preserve left-to-right, exactly-once operand evaluation;
7. preserve non-associative comparison parsing; and
8. keep comparison results in the existing normalized Bool family.

## Operator table

| Operator | Meaning | Accepted operands |
|---|---|---|
| `==` | equal | existing equality domain |
| `!=` | not equal | existing equality domain |
| `<` | less than | matching Int, Float, or String |
| `>` | greater than | matching Int, Float, or String |
| `<=` | less than or equal | matching Int, Float, or String |
| `>=` | greater than or equal | matching Int, Float, or String |

`!=` is the exact Boolean complement of `==`. It does not introduce a second
equality protocol. Function values therefore remain equal only by opaque
function identity, and compound values retain existing structural equality.

Ordered comparison does not derive from structural equality and does not add
an ordering protocol to other values.

## Grammar and precedence

The comparison production becomes:

```text
expression ('==' | '!=' | '<' | '>' | '<=' | '>=') expression
```

All comparison operators have lower precedence than arithmetic and higher
precedence than `&&`. They do not associate:

```telora
a < b && b <= c       # valid
(a < b) == enabled    # valid
a < b <= c            # parse error
a == b != c           # parse error
```

Parentheses are required when the result of one comparison is intentionally
used as an operand of another.

The lexer recognizes the longest token first. `!=`, `<=`, and `>=` are single
comparison tokens, not a `!`/`<`/`>` token followed by `=`.

## Static semantics

### Equality

`==` and `!=` accept the same operands as existing equality. They do not
require numeric or ordered types. Their result is Bool.

The checker does not use `!=` to refine types or prove pattern exclusion.

### Ordered comparison

Both operands of `<`, `>`, `<=`, or `>=` must resolve to the same ordered
primitive type:

```text
Int x Int       -> Bool
Float x Float   -> Bool
String x String -> Bool
```

Mixed numeric comparison remains invalid. In particular, `Int` is not
implicitly promoted to `Float`. String is not coerced to or from any other
type.

Expected types and opposite operands may constrain an inference variable. An
otherwise unconstrained ordered parameter remains ambiguous and must not be
generalized or erased to `Any` merely because three ordered domains exist.

`Any` retains its explicit dynamic-boundary behavior. At runtime, operands
must still be a matching accepted pair.

## Int semantics

Int comparison uses signed mathematical order over Telora's `i64` value range.
It cannot overflow and has the usual complement relationships:

```text
a != b  == !(a == b)
a > b   == b < a
a >= b  == b <= a
```

These identities describe results, not source rewriting or evaluation order.

## Float semantics

Float comparisons follow Rust `f64` primitive operations and IEEE unordered
behavior:

- `NaN == value` is false, including when `value` is NaN;
- `NaN != value` is true;
- `<`, `>`, `<=`, and `>=` are false if either operand is NaN;
- positive and negative zero compare equal; and
- infinities compare in their ordinary numeric order.

Telora does not promise total ordering of Float. This RFC does not introduce a
NaN payload order, bitwise Float equality, or a `total_cmp` operator.

Consequently, `<=` and `>=` are primitive ordered comparisons. They are not
defined as negated reverse `<` expressions.

## String semantics

String ordered comparison is lexicographic over Telora's internal UTF-8 byte
sequence. At the first differing byte, the lower byte sorts first. If one byte
sequence is a prefix of the other, the shorter String sorts first.

Examples:

```telora
"apple" < "banana"       # 'True
"app" < "apple"          # 'True
"10" < "2"               # 'True
"Z" < "a"                # 'True
```

The internal encoding is the semantic authority for this operation, not an
abstract Unicode collation model. Telora Strings contain valid UTF-8, so this
order also agrees with Unicode scalar-value order for valid String contents,
but that equivalence is not a separate language mechanism.

Comparison operates on exact decoded contents:

- no Unicode normalization is performed;
- canonically equivalent but differently encoded scalar sequences remain
  distinct;
- no locale collation is consulted;
- no case folding occurs; and
- digit runs are not interpreted as numbers.

These rules match existing exact String equality. A future standard-library
collator may expose normalization, locale, or natural ordering explicitly.

## Evaluation and runtime semantics

The left operand evaluates first, then the right operand, each exactly once.
An implementation may reuse internal operations by swapping already evaluated
registers for `>` or `>=`; it may not swap source evaluation order.

Comparison creates only the canonical inline Bool result and consumes ordinary
expression execution. It allocates no Telora heap object and introduces no
additional fuel charge beyond existing control-flow and call rules.

Runtime ordered comparison accepts only:

- Int with Int;
- Float with Float; or
- String with String, including either inline or heap-backed String storage.

Every other pair is a sourced type mismatch at a dynamic boundary.

## Diagnostics and tooling

Lexer, lossless CST, parser, HIR, semantic facts, compiler origins, and runtime
errors retain the complete authored comparison expression and operator range.

Invalid static operands report that ordered comparison requires matching Int,
Float, or String operands. Non-associative chains continue to report the
focused instruction to add parentheses.

Formatters and editor tooling must preserve the authored operator. There is no
canonical source rewrite from `>=` to another spelling.

## Compatibility

This change is additive for previously valid programs. The relevant token
prefixes either already have separate lexical roles or occur only inside
longer punctuation such as `->` and `=>`; longest-token recognition does not
reinterpret any valid existing token sequence. Previously invalid sequences
such as `a <= b` become valid comparisons.

Extending `<` from numeric operands to matching String operands is additive.
Numeric inference, equality behavior, and runtime value representation remain
unchanged.

## Implementation plan

1. Add lossless lexer and grammar tokens for `!=`, `>`, `<=`, and `>=` at the
   existing comparison precedence.
2. Extend AST comparison operators and parser non-associativity checks.
3. Add an ordered inference constraint accepting only Int, Float, or String,
   without weakening numeric arithmetic constraints.
4. Lower all comparisons while preserving left-to-right operand evaluation.
5. Add the minimum internal bytecode support needed for inequality and
   inclusive ordering; reuse swapped evaluated registers where semantics agree.
6. Extend runtime comparison to inline/heap String representations and exact
   Float primitive behavior.
7. Update the language SSOT and add lexer, parser, inference, compiler, VM,
   dynamic-boundary, Unicode, NaN, and source-origin tests.
8. Update this RFC to `Implemented` with the observed result.

## Acceptance criteria

1. All six operators parse, retain their source tokens, and return Bool.
2. All comparison operators share one non-associative precedence level.
3. Int boundary and equality truth tables pass.
4. Float tests cover ordinary values, infinities, signed zero, and NaN.
5. String tests cover prefixes, ASCII digits/case, non-ASCII UTF-8 byte order,
   and inline versus heap-backed storage.
6. Equality and inequality retain structural and function-identity semantics.
7. Mixed or unsupported ordered operands fail without `Any` degradation.
8. Dynamic operand errors retain the authored expression and operator.
9. Left and right operands evaluate once in source order.
10. Existing workspace behavior passes formatting, strict Clippy, and the full
    test suite.

## Non-goals

- locale-aware or language-aware collation;
- normalization or case-insensitive comparison;
- natural sorting of embedded digit runs;
- ordering for Bytes, Atom, Bool, Array, Tuple, Dict, Struct, Enum, Function,
  TypeMetadata, Dyn, or opaque values;
- implicit Int/Float coercion;
- chained comparison syntax;
- user-defined ordering, traits, or operator overloading;
- a total Float order; and
- sorting collection APIs.

## Rejected alternatives

### Keep only `<` and `==`

The core is theoretically small but repeatedly forces inverted branches in
ordinary programs and generated eDSL code. The missing surface has direct A2
evidence and no corresponding simplification for readers.

### Specify inclusive comparisons as negated reverse `<`

This gives incorrect results for Float NaN. It would also make the observable
semantics depend on a hidden rewrite rather than the accepted primitive
comparison domain.

### Add locale-aware String operators

Locale collation is environment-dependent, configurable, and often
application-specific. A foundational operator must produce the same result in
tool stage, runtime, tests, and hosts.

### Order every structurally comparable value

Structural equality does not imply one canonical domain order. Ordering tags,
records, functions, metadata graphs, or opaque host values would introduce
arbitrary policy and future compatibility constraints without A2 pressure.
