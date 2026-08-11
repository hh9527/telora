# RFC 0222: Array Indexing, Tuple Projection, and Marked Type Application

- Status: Proposed
- Depends on: RFC 0003, RFC 0021, RFC 0052, RFC 0077, RFC 0081, RFC 0127,
  RFC 0189, RFC 0219, RFC 0220

## Summary

Add two postfix projection forms:

```telora
array[index]  # Array(A) x Int -> A
tuple.0       # Tuple([A, B, ...]) -> A
```

Array indexing is partial access. It returns the selected element directly.
When the index is negative or outside the Array, evaluation raises sourced
blame equivalent to the failure path:

```telora
fail!("OutOfRange", array, index)
```

The existing `array.get(array, index) -> Option(A)` remains the total access
operation. The two interfaces deliberately serve different control-flow
needs.

Tuple projection uses a non-negative decimal source literal. For a statically
known Tuple, its index is checked during analysis and its result is the exact
item type.

Explicit type application moves to a marked postfix form:

```telora
identity@[Int](1)
```

The three projection/application forms are locally distinct:

```text
expression@[types]  static generic type application
expression[index]   runtime Array indexing
expression.N        static positional Tuple projection
```

The parser classifies them directly from punctuation. Type application remains
tool-stage metadata application erased from runtime bytecode; indexing and
Tuple projection are runtime value expressions.

## Motivation

Telora has named structural projection and total Array access, but lacks direct
positional projection:

```telora
record.name
array.get(values, index)
match pair { (left, right) => left }
```

Direct indexing is useful when absence is an invariant violation rather than a
value the caller intends to branch on. Requiring every such access to unpack an
Option obscures the intended successful type. Conversely, changing
`array.get` to fail would remove the total operation needed by parsers,
searches, and value-level validation. As in Rust's `get` and `index` split,
both operations can coexist with distinct, explicit failure contracts.

Tuple item positions are part of the Tuple type itself. A literal projection
therefore has no ordinary absence case and can preserve a more precise result
than a dynamic collection operation. It is particularly useful for values from
`array.enumerate`, `array.zip`, Dict enumeration, and small internal products;
today callbacks must introduce a match solely to name one component.

The opencode test-10 A2 experiment independently attempted Array subscript
syntax while implementing ordered graph traversal. RFC 0220 supplied total
access and enumeration, but direct partial projection remains a general
language operation rather than an ontology-specific request.

Square brackets already denote explicit generic application. RFC 0077 relied
on the absence of value indexing to make that spelling syntactically unique.
Giving unmarked brackets a second phase-dependent meaning would force syntax,
HIR, recovery, and tooling to defer classification until scheme resolution.
The `@` marker keeps the existing TypeMetadata argument list while making its
static phase explicit and reserving unmarked brackets for runtime indexing.

## Goals

1. provide concise partial Array indexing with an exact element result;
2. preserve `array.get` as total Option-returning access;
3. express out-of-range indexing through existing structured blame;
4. preserve left-to-right, exactly-once operand evaluation and provenance;
5. provide exact, statically checked positional Tuple projection;
6. make explicit type application locally distinct as `f@[T]`;
7. classify type application and indexing directly from punctuation;
8. keep indexing immutable and free of assignment semantics; and
9. preserve recoverable syntax, HIR, facts, hover, and source diagnostics.

## Surface Syntax

The postfix expression family accepts:

```text
type_application   := expression '@' type_arguments

type_arguments     := '[' type_argument
                      (',' type_argument)* [','] ']'

type_argument      := expression | '_'

array_index        := expression '[' expression ']'

tuple_projection   := expression '.' Int
```

All three forms retain call-level postfix precedence. Tuple projection has the
same precedence and associativity as named field projection. Examples:

```telora
matrix[row][column]
pairs[index].0
outer.1.name
generic@[Item](input)[index]
```

The lexer continues to recognize `1.0` as one Float token. In `pair.0`, the
receiver, dot, and Int selector are distinct tokens. Whitespace does not change
that tokenization.

Tuple selectors are unsigned decimal literals. Negative selectors, computed
selectors, and placeholders are not Tuple projection syntax:

```telora
pair.0       # positional Tuple projection
pair.-1      # invalid
pair.index   # named field projection, not positional projection
pair[index]  # Array indexing and therefore invalid for a known Tuple
```

## Explicit Type Application

The `@` marker is part of the type-application expression. Its bracket contents
retain the exact RFC 0077 and RFC 0081 semantics: every argument is a
TypeMetadata expression checked and evaluated at tool stage, the receiver must
statically identify a generic `TypeScheme`, and the resulting monomorphic value
is erased to the receiver at runtime.

Complete and partial applications are written:

```telora
identity@[Int](1)
pair@[Int, _](1, "value")
```

First-class specialization remains valid:

```telora
let int_identity = identity@[Int];
int_identity(1)
```

Unmarked square brackets always denote Array indexing:

```telora
generic@[Int]  # type application
values[index] # Array indexing
types[index]  # Array(Type) indexing when index is Int
```

Unmarked square brackets have only the Array-indexing meaning:

```telora
identity[Int] # invalid because identity is not an Array
```

There is no compatibility grammar, semantic fallback, legacy classification,
or special migration branch for the old spelling. It is an ordinary indexing
type error under the new language.

Prefix `@decorator` syntax remains distinct. A decorator marker precedes the
declaration or field it transforms, while the type-application marker follows
its receiver expression and is immediately followed by a type-argument list.

## Array Indexing

### Static semantics

For a receiver `Array(A)` and an index `Int`:

```text
Array(A) x Int -> A
```

The receiver is inferred first. The index is checked against Int. An unresolved
ordinary value receiver may acquire an `Array(A)` constraint; this does not
make runtime function values type-applicable or introduce an index trait.

Known non-Array receivers are static errors. `Dyn` does not authorize indexing.
An explicitly erased `Any` receiver produces `Any`; runtime checks still
require an Array and Int index.

Indexing does not refine the receiver length or introduce a proof that a later
index is present. A successful access produces the element type only on that
execution path.

### Evaluation

The receiver evaluates first, followed by the index, each exactly once. Their
evaluated values are retained internally for both lookup and failure subjects.
The semantic failure path is:

```telora
let internal_array = array_expression;
let internal_index = index_expression;

match array.get(internal_array, internal_index) {
    'Some(value) => value,
    'None => fail!("OutOfRange", internal_array, internal_index),
}
```

This expansion defines value and failure behavior, not resource accounting or
a required source rewrite. Implementations must not evaluate either authored
operand again while constructing blame.

When `0 <= index < array.length(array)`, the result is the existing rich element
value. Its data provenance is unchanged. A negative index and an index greater
than or equal to the length both raise the same out-of-range blame.

At an `Any` boundary, a non-Array receiver or non-Int index is a sourced dynamic
type mismatch. It is not reported as `OutOfRange`, because no valid Array
position was tested.

### Structured failure

The out-of-range path is exactly the convenience failure:

```telora
fail!("OutOfRange", internal_array, internal_index)
```

Equivalently:

```telora
raise!(blame!("OutOfRange", internal_array, internal_index))
```

The raised value is the existing `BlameError`, and the runtime failure kind is
the existing `RaisedBlame`. This RFC does not add an `IndexOutOfBounds` runtime
failure class.

The complete authored `array[index]` expression is the rule origin. The Array
and index retain their separate data origins as blame subjects. Rendering and
diagnostic collection use the existing bounded BlameError rules.

### Resource semantics

Successful indexing reads the existing element directly. It creates no Option
wrapper, performs no Telora heap allocation, and consumes no native-call fuel.
This intentionally differs from literally calling `array.get` and immediately
unwrapping its allocated `'Some` result.

The failure path constructs and raises the ordinary BlameError under existing
allocation, stack, trace, and diagnostic rules. Reading the Array and checking
the index introduces no traversal fuel proportional to Array length.

### Relationship to `array.get`

Both operations use zero-based Int positions and agree on presence:

```telora
array.get(values, index) # Option(A), total
values[index]            # A, raises blame when absent
```

`array.get` remains useful as a first-class generic function, in callbacks, and
whenever absence is expected control flow. Indexing is syntax and cannot be
passed as a function value without an explicit closure.

## Positional Tuple Projection

### Static semantics

For a known Tuple descriptor:

```text
Tuple([T0, T1, ..., Tn]).i -> Ti
```

The source selector is converted to a host index with checked conversion. If it
does not fit a host index or is greater than or equal to the known Tuple length,
analysis reports a source error at the selector. No bytecode is emitted for a
statically invalid projection.

For a Union receiver, every reachable member must be a Tuple containing that
position; the result is the canonical join of the projected item types. `Never`
members do not contribute. A known non-Tuple member rejects the projection.

An explicitly erased `Any` receiver produces `Any` and is checked at runtime.
`Dyn` does not authorize positional projection. An unresolved receiver does not
acquire an open or variadic Tuple constraint; it must become a known Tuple from
other evidence or an explicit contract before its inference boundary closes.

### Evaluation and dynamic boundaries

The receiver evaluates exactly once. Projection returns the existing rich item
without allocation and preserves its data provenance. The complete authored
`tuple.0` expression is its rule origin for failures.

At an `Any` boundary, a non-Tuple receiver is a sourced dynamic type mismatch.
If the runtime Tuple is shorter than the literal selector, evaluation raises:

```telora
fail!("OutOfRange", tuple, index)
```

where `index` is the selector converted to an Int subject located at the
selector token. This dynamic out-of-range case cannot occur for a statically
known Tuple.

The implementation may reuse the existing Tuple item bytecode operation used
by pattern destructuring. A user-authored dynamic out-of-range projection is a
recoverable `RaisedBlame`, not invalid bytecode.

## Assignment and Immutability

Both forms are read-only expressions. They are not assignment targets:

```telora
values[index] = replacement # invalid
pair.0 = replacement        # invalid
```

This RFC adds no mutation, mutable references, lvalue category, in-place
replacement, or copy-update syntax. Programs construct changed Arrays and
Tuples through ordinary immutable operations.

## Tooling and Recovery

Lossless syntax and HIR preserve three distinct nodes directly from
punctuation:

- explicit type application;
- Array indexing; and
- positional Tuple projection.

Type-application arguments retain TypeMetadata facts and tool-stage evaluation.
An Array index retains an ordinary runtime Int fact and participates in value
references. Hover reports the monomorphic specialized Function for type
application, the element type for indexing, and the selected item type for
Tuple projection.

Completion after `tuple.` may offer numeric positions only when the receiver is
a known Tuple. Named field completion remains unchanged. Definition and
reference navigation within receiver and index expressions follows ordinary
expression rules.

Recovery knows the expression category from punctuation without resolving a
binding or evaluating TypeMetadata. Missing semantic evidence keeps downstream
facts unavailable without changing the expression category or inventing `Any`.

Formatters preserve authored square brackets and numeric selectors. They do
not rewrite indexing to `array.get`, Tuple projection to match, or type
application to another delimiter.

## Diagnostics

Required focused diagnostics include:

- indexing requires exactly one index expression;
- Array index must be Int;
- indexing receiver must be an Array;
- type-application receiver must identify a generic scheme;
- type-application arguments must be TypeMetadata;
- Tuple projection requires a Tuple receiver;
- Tuple selector is outside the statically known length;
- dynamic indexing requires an Array and Int; and
- dynamic positional projection requires a Tuple.

Out-of-range Array access and dynamic Tuple projection use the specified
`OutOfRange` BlameError instead of a plain diagnostic string. Static Tuple
bounds errors remain analysis diagnostics because the program cannot reach a
valid execution for that source form.

Diagnostics retain the complete bracket or projection expression and focus
secondary labels on the receiver, index, or selector responsible for the
failure.

## Compatibility

Explicit type application changes spelling without a compatibility period:

```telora
generic[T]  # invalid indexing expression
generic@[T] # explicit type application
```

All valid complete and partial applications, including first-class
specialization, must add the `@` marker. Their TypeMetadata evaluation,
substitution, static result, facts, and bytecode erasure otherwise remain
unchanged.

Unmarked square brackets are uniformly Array indexing. Existing source that
uses them on a generic scheme is invalid because the receiver is not an Array;
the implementation does not recognize it as a legacy type application.

The new `.` followed by Int form was previously invalid. It does not capture
named fields because field names are Identifier tokens, and it does not split
Float tokens.

## Implementation Plan

1. Extend lossless grammar and typed CST views with marked `@[...]` type
   application, Array indexing, and numeric postfix projection.
2. Add distinct TypeApply, ArrayIndex, and TupleProjection AST/HIR nodes directly
   from punctuation.
3. Move complete and partial explicit type application to the marked syntax
   while preserving its analysis, tool-stage evaluation, and runtime erasure.
4. Add Array constraints, Int index checking, exact element facts, and Tuple
   positional projection to strict and recovery analysis.
5. Add direct Array index LIR/bytecode/VM execution with rich element retention,
   dynamic checks, and structured out-of-range blame.
6. Reuse Tuple item execution while separating authored out-of-range blame from
   malformed-bytecode failure.
7. Migrate repository source, standard modules, examples, tests, and generated
   snippets from `[T]` to `@[T]` without accepting both forms.
8. Update the language SSOT, `tutorial.md`, and
   `experiments/ontology-edsl/TELORA-TUTORIAL.md` only after implementation.
   The ontology tutorial must teach `generic@[T]`, `array[index]`, `tuple.N`,
   and the total/partial distinction between `array.get` and indexing.
9. Record the changed ontology tutorial hash and require the next ontology eDSL
   experiment to identify itself as a new input baseline.
10. Add lexer, CST, parser, inference, compiler, VM, provenance, quota, recovery,
    LSP, formatter-facing, and diagnostic tests.

## Acceptance Criteria

1. `values[0]`, `values[index]`, and chained indexing return the exact Array
   element type and value.
2. Receiver and index evaluate once in left-to-right order.
3. Negative, equal-to-length, and greater-than-length indices raise
   blame equivalent to `fail!("OutOfRange", values, index)`, with the bracket
   expression as rule origin and both operands as subjects.
4. Successful indexing retains element provenance and allocates no Option or
   other wrapper.
5. `array.get` retains its exact Option-returning, allocation, provenance, and
   quota semantics.
6. Indexing known non-Arrays and non-Int indices fails statically; erased
   dynamic mismatches fail at runtime with sourced type diagnostics.
7. `pair.0` and nested/chained projections return the exact item type and value.
8. Known Tuple bounds failures are static; `Any` boundary bounds failures raise
   sourced `OutOfRange` blame.
9. Tuple projection preserves item provenance and allocates no result wrapper.
10. Complete and partial `generic@[T]` applications retain existing analysis,
    tool-stage evaluation, hover facts, and bytecode erasure.
11. A first-class specialization such as `let f = generic@[T]; f(value)` is
    valid.
12. Unmarked `generic[T]` is never accepted as type application, while a runtime
    `Array(Type)` uses `types[index]` as ordinary indexing.
13. Recovery distinguishes all three forms from punctuation and does not
    publish `Any` as a strict fact when semantic evidence is unavailable.
14. Float lexing, named field projection, Tuple patterns, and formatter output
    remain unchanged.
15. The language SSOT, `tutorial.md`, and the injected ontology eDSL tutorial
    describe only `generic@[T]` for type application, `array[index]` for partial
    access, `array.get` for total access, and `tuple.N` for positional Tuple
    projection.
16. The next ontology eDSL run records the new tutorial hash and does not claim
    verbatim comparability with runs using the prior input.
17. The full workspace passes formatting, strict Clippy, and all tests.

## Non-goals

- indexed assignment or any other mutation;
- slicing, ranges, insertion, replacement, or removal;
- negative-from-end positions;
- dynamic Tuple indexing;
- named Tuple items;
- user-defined indexing, traits, or operator overloading;
- indexing Dict, String, Bytes, Dyn, or arbitrary user values;
- changing Array storage or asymptotic access guarantees; and
- alternative or compatibility spellings for explicit type application.

## Rejected Alternatives

### Return Option from `array[index]`

The total operation already exists as `array.get`. Direct indexing is useful
precisely when presence is required and the successful expression should have
type `A`. Returning Option from both surfaces would add notation without adding
a distinct control-flow contract.

### Add an unstructured bounds runtime error

Telora already represents authored, sourced failures with BlameError. Raising
`fail!("OutOfRange", array, index)` retains both subjects and composes with the
existing VM/Host failure boundary. A new error kind would bypass that model and
carry less authored evidence.

### Keep unmarked type application and classify brackets semantically

The analyzer could treat `receiver[argument]` as type application when the
receiver identifies a generic scheme and as indexing otherwise. That preserves
existing source but moves a tool-stage/runtime distinction behind binding
resolution. Syntax recovery, HIR, facts, formatter behavior, and diagnostics
would all carry an unresolved bracket category. The explicit `@` marker makes
the distinction local and removes that cross-phase classification.

### Parse brackets using capitalization or argument syntax

TypeMetadata arguments are ordinary computed expressions, and runtime bindings
may use any valid identifier. Capitalization, whether a call follows, or the
apparent shape of an argument cannot define the phase of a bracket expression.

### Let the parser resolve names for unmarked brackets

Parser-time symbol lookup would couple lossless syntax to module resolution and
make damaged documents unstable. Punctuation should determine the expression
category before scheme identity or runtime constraints are available.

### Use type inference to choose between two unmarked meanings

Trying both interpretations and selecting the one that checks would make a
local type edit move bracket arguments between tool-stage and runtime
evaluation. Marked type application keeps diagnostics and evaluation phase
deterministic.

### Add only `tuple.0`

Tuple projection is independently useful and easier to implement, but it does
not address required access to homogeneous runtime positions. The two forms
complete the named, static-positional, and dynamic-positional projection
surface while sharing postfix precedence and tooling work.

### Use method syntax

`array.get(values, index)` already provides a named total function. Adding
`values.get(index)` would require a separate method-resolution or uniform-call
mechanism and still would not provide the conventional required-access
operator.
