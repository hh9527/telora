# RFC 0223: Finite Float Domain and Non-finite Failure

- Status: Implemented
- Depends on: RFC 0003, RFC 0021, RFC 0052, RFC 0105, RFC 0127,
  RFC 0187, RFC 0189, RFC 0221
- Tracking issue: #32

## Summary

Define Telora `Float` as the finite subset of IEEE 754 binary64. `NaN`,
positive infinity, and negative infinity are not Telora values.

Every Telora Float computation checks its result before publication. If the
result is non-finite, evaluation raises sourced blame equivalent to:

```telora
fail!("NonFiniteFloat", operands...)
```

For a binary operator, the two already evaluated operands are blame subjects
in source order. The complete authored operator expression is the rule origin.
Operands evaluate left to right and exactly once.

External values are checked when they enter the Telora value domain. TOML,
YAML, Host/native, regex parsing, `Dyn`, module import, and publication paths
must reject a non-finite Float at the earliest authoritative boundary and
retain its source or boundary provenance.

This is a breaking restriction of the current language. It replaces the
existing behavior in which Float division and static-data inputs can produce
or contain non-finite values.

## Motivation

Telora uses deterministic evaluation to construct models, diagnostics, and
reusable plans. Full IEEE Float admits values that do not fit the language's
ordinary value laws or publication boundaries:

- NaN is not equal to itself and is unordered;
- NaN has multiple payload bit patterns without Telora-level semantics;
- platform and operation details can affect those payloads;
- infinity is ordered, but arithmetic on infinities can produce NaN;
- NaN and infinities are not representable in JSON; and
- silently propagated non-finite results move an error away from the operation
  that produced it.

The current language already rejects non-finite Float when rendering a Telora
literal or encoding JSON, but permits it during evaluation and through TOML
and YAML. That makes validity depend on a later consumer rather than on the
Float value itself.

A finite value domain restores a simple invariant:

```text
every value whose Telora type is Float is a finite binary64 value
```

The invariant makes structural equality, ordering, codecs, hashing, module
publication, and future deterministic artifacts operate over one closed public
domain. It also reports arithmetic failure at its authored cause.

## Goals

1. define Float as finite IEEE 754 binary64;
2. reject every computed NaN or infinity at the producing expression;
3. express computation failure through existing sourced `fail!` semantics;
4. reject non-finite external values before they become Telora values;
5. preserve operand evaluation order, provenance, quotas, and traces;
6. preserve finite binary64 arithmetic and signed zero;
7. make JSON and other finite-number codecs total for valid Float values; and
8. provide the Float-domain prerequisite for a future remainder operator.

## Non-goals

This RFC does not:

- add decimal, rational, arbitrary-precision, or checked-result numeric types;
- add source literals for NaN or infinity;
- expose NaN payloads or binary64 bit patterns;
- add implicit Int/Float conversion;
- add `%`; RFC 0223 only defines the Float domain on which `%` may later work;
- make ordinary arithmetic return `Result`; or
- change Int overflow and division rules.

## Float value domain

A Telora Float is an IEEE 754 binary64 value for which `is_finite()` is true.
The domain includes:

- positive and negative normal values;
- positive and negative subnormal values;
- positive zero; and
- negative zero.

The domain excludes:

- every quiet or signaling NaN representation;
- positive infinity; and
- negative infinity.

Positive and negative zero remain distinct binary64 encodings but retain the
existing language equality and ordering behavior:

```telora
-0.0 == 0.0  # 'True
```

This RFC does not expose a Float bitwise-equality operation. A future stable
binary encoding may preserve or canonicalize signed zero, but must specify that
choice independently.

## Float literals

Decimal Float literals must parse to a finite binary64 value. A literal whose
magnitude rounds to infinity is invalid source and receives a diagnostic at
the literal. There are no NaN or infinity keywords or literal spellings.

Float literals accept decimal-point notation (`1.25`) and exponent notation
with or without a decimal point (`1e308`, `1.25e-3`, `1.0E+8`). A leading sign
remains an ordinary unary operator rather than part of the literal token.

Literal rejection is an analysis error, not a runtime `fail!`, because no
valid executable Float operand exists.

## Computation semantics

### General rule

For every operation that computes a Telora Float:

1. evaluate authored operands in source order, exactly once;
2. perform the specified binary64 operation;
3. test the result with the authoritative finite-value predicate;
4. return the result when finite; otherwise
5. raise blame equivalent to `fail!("NonFiniteFloat", operands...)`.

The non-finite intermediate is never installed in a Telora register, heap
value, collection, binding, module export, diagnostic subject, or result.

### Existing arithmetic

The rule applies to Float `+`, `-`, `*`, and `/`.

Examples of failure include:

```telora
1.0 / 0.0       # NonFiniteFloat
-1.0 / 0.0      # NonFiniteFloat
0.0 / 0.0       # NonFiniteFloat
largest * 2.0   # NonFiniteFloat when binary64 overflows
```

Float division by positive or negative zero uses `NonFiniteFloat`, not the Int
`DivisionByZero` runtime kind. The semantic reason is that the Float operation
failed to produce a member of the Float domain. This keeps all non-finite Float
results on one structured failure path.

Unary negation of a valid finite Float is always finite. It remains subject to
the invariant check at shared implementation boundaries, but cannot raise
`NonFiniteFloat` from a valid Telora operand.

### Future Float operations

Every future operator, intrinsic, standard-library function, interpreter, or
native capability returning Float must obey the same postcondition.

If `%` is accepted, finite Float remainder uses the chosen binary64 remainder
operation and then applies this RFC's finite-result check. A zero divisor or
any other non-finite result raises `NonFiniteFloat`. The `%` RFC remains
responsible for precedence, negative operands, and exact remainder semantics.

## Structured failure

For a binary operation `left op right`, non-finite failure is semantically:

```telora
let internal_left = left;
let internal_right = right;
fail!("NonFiniteFloat", internal_left, internal_right)
```

Equivalently:

```telora
raise!(blame!("NonFiniteFloat", internal_left, internal_right))
```

This expansion defines value, provenance, and diagnostic behavior. It is not a
required source rewrite and does not permit repeated operand evaluation.

The failure uses the existing `BlameError` value and `RaisedBlame` runtime
path. It does not add a separate terminal `NonFiniteFloat` runtime error kind.
The complete authored computation is the rule origin. Operand values retain
their individual data origins and appear as heterogeneous blame subjects in
source order.

The failure message is exactly `NonFiniteFloat`. Diagnostics must not include
NaN payload bits, platform-specific Float formatting, or the rejected
intermediate value.

## External and internal boundaries

The finite invariant must be enforced at every authority capable of creating
or importing a Float.

### Static data

JSON already has no non-finite number syntax. TOML `inf`, `+inf`, `-inf`,
`nan`, `+nan`, and `-nan`, and YAML `.inf`, `-.inf`, and `.nan` forms are
rejected when materializing a Telora Float. Diagnostics point to the data
scalar and identify that Telora Float requires a finite value.

These are data-validation diagnostics rather than `fail!`: the invalid value
never enters expression evaluation. Independent recoverable workspace facts
continue under existing partial-analysis rules.

### String and regex parsing

Any parser producing Float accepts only text that parses to a finite binary64
value. A syntactically valid non-finite or overflowing representation is a
parse failure through that provider's existing `Result` or blame protocol.

### Host, native, and opaque boundaries

Every public API that constructs, returns, imports, copies, projects, or
publishes a Telora Float validates finiteness. The primary construction API
must reject non-finite values before storing them. Heap import, module export,
world promotion, and publication retain defensive checks so an untrusted or
incorrect Host implementation cannot violate the invariant.

A native computation invoked by a Telora expression that attempts to return a
non-finite Float raises `NonFiniteFloat` at the authored native call, with the
native call's available operands as subjects. A Host injection or static
module load without an authored computation reports a sourced boundary error
instead.

### `Any`, `Dyn`, and codecs

`Any` and `Dyn` erase static shape, not value validity. Packing, unpacking, or
checking a dynamic value cannot admit a non-finite Float.

All valid Telora Float values are JSON-number encodable. JSON encoding retains
a defensive finite check for corrupted or untrusted internal state, but that
check is unreachable for a valid Telora program. Other codecs must not invent
special strings or null values for non-finite numbers.

## Equality, ordering, hashing, and display

RFC 0221's NaN and infinity clauses are superseded for Telora values. Float
equality and ordered comparison operate only on finite operands:

- equality is reflexive;
- positive and negative zero compare equal;
- `<`, `>`, `<=`, and `>=` have ordinary finite numeric behavior; and
- no unordered comparison result exists.

The language still does not promise a bitwise order or expose a `total_cmp`
operator. Signed zero means the comparison relation is a total preorder over
encodings and an ordinary total order over Telora's observable numeric values.

Float display and interpolation only receive finite values. Hashing, cache
keys, persistent artifacts, and deterministic binary encodings may therefore
define stable finite-number behavior without a NaN payload policy. They must
still specify signed-zero handling when bit-level identity matters.

## Evaluation and resource semantics

Finite-result validation is part of each Float-producing operation. It adds no
Telora heap allocation and no call or back-edge fuel charge on success.

The failure path constructs and raises the ordinary `BlameError` under existing
allocation quota, stack, trace, reporting, and diagnostic rules. Insufficient
resources while constructing the failure retain the existing quota semantics.

Compiler constant folding, tool-stage evaluation, runtime bytecode execution,
native execution, and recoverable workspace evaluation must make the same
finite/non-finite decision and preserve the same authored origin.

## Compatibility and migration

This RFC is intentionally breaking. The following currently accepted behavior
becomes invalid:

```telora
0.0 / 0.0  # currently NaN; becomes NonFiniteFloat
1.0 / 0.0  # currently +Inf; becomes NonFiniteFloat
```

TOML and YAML non-finite scalars currently materialize Float and will instead
produce data diagnostics. Tests and documentation that use NaN or infinity to
exercise comparison behavior must be removed or rewritten as finite cases.

Migration rules:

1. computations using infinity as a sentinel must use an explicit Option,
   Enum, or domain-specific tagged value;
2. computations relying on NaN propagation must represent failure explicitly;
3. external non-finite data must be rejected or normalized before the Telora
   boundary according to an application-owned policy; and
4. no compatibility flag, legacy Float type, or silent clamping is provided.

The language SSOT replaces its IEEE non-finite comparison clauses with the
finite-domain rule when implementation lands. Historical RFC 0221 remains an
accurate record of the earlier decision and is not rewritten.

## Rejected alternatives

### Permit NaN and canonicalize its payload

Canonicalization removes payload nondeterminism but does not restore reflexive
equality, ordering, JSON representation, or cause-local failure.

### Reject NaN but retain infinities

Infinities are ordered and canonical, but ordinary arithmetic on them can
produce NaN and they remain outside JSON. Supporting them would require a
larger partial-operation matrix without solving the publication boundary.

### Preserve IEEE values and fail only during encoding

This reports the problem at a consumer rather than at the producing operation
and makes validity depend on which codec or artifact is requested.

### Clamp overflow to the largest finite value

Clamping silently changes arithmetic and loses the distinction between a
valid boundary result and overflow. It is unsuitable for deterministic model
construction.

### Return `Result(Float, Error)` from arithmetic

Changing all arithmetic result types is a much larger language design. Telora
already has sourced `fail!` for invariant violations, and this RFC uses that
existing control-flow contract.

### Add a separate `FiniteFloat` type

Keeping full IEEE `Float` would continue to expose problematic values through
ordinary arithmetic and generic numeric code. Telora currently has no trait or
numeric hierarchy that would make two Float domains ergonomic or consistently
polymorphic.

## Implementation plan

1. Centralize the authoritative finite-Float predicate and checked value
   construction at VM and heap boundaries.
2. Change Float arithmetic lowering/execution to validate results and raise
   sourced `NonFiniteFloat` blame with already evaluated operands.
3. Reject overflowing Float literals during parsing/validation.
4. Reject TOML and YAML non-finite scalars with precise data provenance.
5. Enforce the invariant in regex/string parsing and every Host/native Float
   construction and return API.
6. Add defensive validation to heap import/copy, `Any`, `Dyn`, module
   promotion/publication, and codec paths.
7. Replace NaN/infinity comparison tests with finite-domain comparison and
   non-finite-failure tests.
8. Update `docs/design/LANGUAGE.md` and
   `experiments/ontology-edsl/TELORA-TUTORIAL.md` when implementation lands.
9. Update the `%` tracking issue to depend on this RFC's Float-domain decision.
10. Run the full workspace tests, strict Clippy, formatting, and deterministic
    source/provenance regression matrix.

## Acceptance criteria

1. No valid Telora runtime, heap, module, dynamic, or published value contains
   a non-finite Float.
2. Float `+`, `-`, `*`, and `/` return their ordinary finite binary64 result or
   raise `NonFiniteFloat` at the authored operation.
3. Binary failure subjects are the finite operands in source order and each
   operand evaluates exactly once.
4. Float division by positive or negative zero uses the same structured
   non-finite failure path.
5. Float overflow is detected consistently in runtime, tool-stage, folded,
   and native computation paths.
6. Float literals that round to infinity are rejected at their source range.
7. TOML, YAML, string/regex, Host/native, `Any`, and `Dyn` boundaries reject
   non-finite values with their available provenance.
8. JSON encoding is total for all valid Float values while retaining a
   defensive invariant check.
9. Finite equality, ordering, signed zero, normal values, and subnormal values
   have stable regression coverage.
10. Existing Int arithmetic, Int division failure, numeric typing, and mixed
    Int/Float rejection are unchanged.
11. The language SSOT and ontology eDSL tutorial describe only the finite Float
    domain after implementation.
12. The full workspace test suite, strict Clippy, formatting, and diff checks
    pass.

## Open implementation audit

Implementation must begin with an audit of every Float constructor and copy
path. At minimum, the audit covers parser literals, compiler constants, VM
arithmetic, heap import/export/copy, static-data materialization, regex/string
parsing, Host/native call APIs, `Any`, `Dyn`, module promotion/publication,
display, interpolation, equality, ordering, JSON, and future hashing/artifact
encoders.

If a boundary cannot preserve sourced `fail!` behavior because it has no
authored expression, it must reject the value before insertion and use that
boundary's existing sourced validation error. It must not fabricate a Telora
expression origin or temporarily store the non-finite value.

## Implementation result

Implemented in the parser, VM, heap boundary, native result API, static-data
loaders, and regex/string parser. Float arithmetic raises `RaisedBlame` with
the exact `NonFiniteFloat` message. Its data location is the first available
operand origin and its rule location is the complete authored operation; the
failure also charges the allocation equivalent of the two-subject
`BlameError`, following the existing direct `OutOfRange` implementation model.

Telora source now accepts finite decimal exponent notation. TOML and YAML
non-finite spellings and numeric overflow are rejected at their scalar source;
JSON retains its finite-number check. Heap import and export reject non-finite
`Value::Float`, which protects module publication, `Any`, `Dyn`, and legacy
Host value boundaries. `CallContext::set_float` rejects non-finite native
results and maps the authored call to `RaisedBlame("NonFiniteFloat")`.

The language SSOT and ontology experiment tutorial now describe only finite
Float comparison. Historical RFC 0221 remains unchanged and its NaN/infinity
clauses are superseded by this RFC.
