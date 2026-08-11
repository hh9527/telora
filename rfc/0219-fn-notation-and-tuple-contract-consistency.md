# RFC 0219: Fn Notation and Tuple Contract Consistency

- Status: Implemented
- Depends on: RFC 0003, RFC 0022, RFC 0050, RFC 0051, RFC 0127, RFC 0203,
  RFC 0218

## Summary

Separate author-facing Function type notation from the ordinary callable that
constructs Function TypeMetadata, and make the real Tuple constructor spelling
composable in every contract position:

```telora
Fn(Input) -> Output       # author-facing Function type notation
Func([Input], Output)     # ordinary TypeMetadata constructor call

Tuple([Left, Right])      # ordinary TypeMetadata constructor call
```

Function notation elaborates exactly to the constructor call:

```text
Fn(P1, ..., Pn) -> R  => Func([P1, ..., Pn], R)
```

`Fn` is syntax only. `Func` is the ordinary two-argument metadata constructor.
`Tuple` remains the ordinary one-argument metadata constructor. The parser no
longer gives `Tuple(A, B)` a context-dependent hidden meaning.

This RFC deliberately does not add `(A, B)` as Tuple type notation. Parentheses
remain ordinary grouping and Tuple value syntax. Tuple types are written
uniformly as `Tuple([A, B])` until a future proposal can define unambiguous
surface notation without reinterpreting first-class TypeMetadata values.

The canonical Function descriptor kind is renamed from `'Function` to `'Func`
at the same boundary. Its `parameters` and `result` payload, inference rules,
assignability, runtime value, VM instructions, and callable ABI do not change.

## Motivation

Telora's TypeMetadata constructors are ordinary callable values. Their intended
source interfaces are:

```telora
Func:  Fn(Array(Type), Type) -> Type
Tuple: Fn(Array(Type)) -> Type
```

The current surface does not preserve that model consistently.

`type` initializers and general annotations use the real Tuple call:

```telora
type Pair = Tuple([Int, String]);
let pair: Tuple([Int, String]) = (1, "value");
```

The restricted grammar for `def`, `decl`, and `native` contracts cannot parse
the same expression. It accepts this instead:

```telora
def pair: Fn() -> Tuple(Int, String) = fn() { (1, "value") };
```

During AST lowering, the parser recognizes the textual callee name `Tuple` and
wraps the apparent arguments in a synthetic Array. `Tuple(Int, String)` looks
like an ordinary two-argument call but is not one. Moving an unchanged type
between an alias and a definition contract either fails to parse or fails at
tool stage with a constructor-arity diagnostic.

The standard native modules use the hidden contract spelling while the
language SSOT specifies `Tuple([...])`. The opencode test-3 A2 experiment hit
this boundary twice and introduced aliases solely to move Tuple results across
the contract grammar.

Function metadata has a related but distinguishable dual surface. The arrow in
`Fn(A) -> B` clearly marks dedicated Function type notation, but the same name
`Fn` is also currently installed as the ordinary metadata constructor. Thus
`Fn` denotes syntax in one context and a callable value in another.

Naming the callable `Func` makes the elaboration boundary explicit and matches
Telora's existing runtime callable vocabulary (`Value::Func`). Renaming the
descriptor kind to `'Func` makes construction, observation, and runtime
vocabulary agree.

Using `Func` rather than `Function` avoids reserving a common domain name. The
repository already contains a SQL-domain enum named `Function`; this RFC does
not force that model to rename an unrelated concept.

## Goals

1. retain readable `Fn(P...) -> R` as the canonical authored Function type
   surface;
2. expose the ordinary Function metadata constructor as `Func`;
3. rename the canonical Function descriptor kind and public observer variant
   from `'Function` to `'Func`;
4. keep `Tuple` as an ordinary constructor whose sole argument is an Array of
   TypeMetadata;
5. accept `Tuple([A, B])` unchanged in every explicit contract and annotation
   position, including nested Function parameters and results;
6. eliminate callee-name-based Tuple rewriting;
7. preserve one TypeMetadata representation and one tool-stage evaluation
   path;
8. preserve ordinary Tuple expression semantics for TypeMetadata values; and
9. give old contextual spellings focused migration diagnostics.

## Vocabulary and authority

This RFC distinguishes:

- **Function type notation**, the authored `Fn(P...) -> R` syntax;
- **notation elaboration**, its source-preserving translation to an ordinary
  `Func` call; and
- **metadata construction**, evaluation of `Func` and `Tuple` with the existing
  tool-stage VM.

The constructors remain authoritative for metadata shape and validation.
Elaboration does not create a second descriptor model or type evaluator.

`Fn` is a reserved notation token. It is not a prelude value and cannot be
called, imported, exported, captured, or shadowed. `Func` has the same prelude
authority and resolution behavior as `Tuple`, `Array`, and the other built-in
metadata constructors.

The two public names have distinct roles:

```text
Fn        authored Function type notation
Func      callable TypeMetadata constructor, canonical descriptor kind,
          and public observer vocabulary
```

## Function type notation

The canonical authored form remains:

```telora
Fn(Input) -> Output
Fn(Left, Right) -> Output
Fn(Array(A), Fn(A) -> B) -> Array(B)
```

It elaborates structurally:

```telora
Func([Input], Output)
Func([Left, Right], Output)
Func([Array(A), Func([A], B)], Array(B))
```

Parameter and result slots accept complete contract metadata expressions. In
particular, Tuple results no longer require aliases:

```telora
def split:
    Fn(Input) -> Tuple([Left, Right]) =
    fn(input) { ... };

def merge:
    Fn(Tuple([Left, Right])) -> Output =
    fn(pair) { ... };

def nested:
    Fn(Fn(A) -> Tuple([B, C])) -> Array(Tuple([B, C])) =
    fn(mapper) { ... };
```

The first argument of the explicit constructor is always an Array:

```telora
Func([Input], Output)  # valid constructor call
Func(Input, Output)    # invalid: Input is not Array(Type)
```

`Fn(Input, Output)` without an arrow is neither Function type notation nor a
compatibility constructor call.

## Tuple metadata consistency

Tuple has one callable interface in every source position:

```telora
Tuple([A, B])
Tuple([])
Tuple([A])
Tuple([A, B, C])
Tuple(array.map(field_types, transform))
```

The Array argument is semantically meaningful. It permits both fixed and
computed heterogeneous products while keeping Tuple a fixed-arity ordinary
callable.

The following examples must parse and evaluate without aliases:

```telora
let value: Tuple([Int, String]) = (1, "one");

type Pair = Tuple([Int, String]);

decl make: Fn() -> Tuple([Int, String]);
def make = fn() { (1, "one") };

def use:
    Fn(Tuple([Int, String])) -> Int =
    fn(pair) {
        match pair { (number, text) => number }
    };

native make_native: Fn() -> Tuple([Int, String]);

let closure = fn() -> Tuple([Int, String]) { (1, "one") };
```

`Tuple(A, B)` is removed. The parser must not infer a constructor protocol from
the textual name of a callee.

## Why parenthesized Tuple type notation is deferred

Telora already uses `(A, B)` as an ordinary Tuple expression. Because
TypeMetadata values are first class, this is valid program data:

```telora
let metadata_pair = (Int, String);
```

Inside programmable metadata code, a helper may deliberately consume such a
Tuple value. Recursively reinterpreting `(A, B)` as `Tuple([A, B])` whenever an
enclosing expression is expected to produce TypeMetadata would change the
arguments of ordinary user functions. Selecting the meaning through type
inference would make annotation evaluation depend circularly on the analysis
that consumes the annotation.

This RFC therefore makes no contextual reinterpretation. Parentheses retain
their existing expression meaning, and `Tuple([A, B])` remains explicit. A
future Tuple notation proposal must supply an unambiguous syntax or a bounded
grammar that preserves ordinary Tuple-of-TypeMetadata values.

`Cons(A, B)` is also not introduced. `Cons` conventionally denotes recursive
head/tail structure and would misdescribe Telora's flat, variadic Tuple
descriptor.

## Elaboration semantics

Function notation elaboration is a source-preserving frontend operation before
tool-stage evaluation:

```text
FunctionType(parameters, result)
    -> Call(Func, [Array(parameters), result])
```

Authored parameter and result expressions retain their own locations. The
synthetic Array and constructor-call shell use the location of the `Fn` syntax.

The output uses the same expression nodes, name resolution, HIR references,
bytecode compilation, VM calls, fuel accounting, provenance, and metadata
decoder as an explicitly authored `Func` call.

Elaboration does not select types, solve generic variables, evaluate metadata,
or inspect constructor results. It is syntactic and may run before strict
inference. Failed constructor resolution or evaluation follows the ordinary
TypeMetadata diagnostic path.

Tuple receives no notation elaboration. `Tuple([A, B])` is parsed and evaluated
as the ordinary one-argument call the author wrote.

## Grammar boundary

The current restricted contract grammar must admit the real Array-bearing
Tuple expression as a nested contract item. Conceptually:

```text
function_contract := "Fn" "(" [contract_item ("," contract_item)* [","]]
                     ")" "->" contract_item
contract_item      := function_contract | metadata_contract
metadata_contract  := Identifier
                     ["(" [contract_argument ("," contract_argument)* [","]]
                     ")"]
contract_argument  := contract_item | metadata_array
metadata_array     := "[" [contract_item ("," contract_item)* [","]] "]"
```

The concrete grammar may share expression productions instead. It must retain
these properties:

1. `Tuple([A, B])` is parseable in Function parameters and results;
2. nested calls such as `Array(Tuple([A, B]))` remain compositional;
3. arbitrary family applications such as `Box(A)` remain parseable;
4. nested Function notation retains its explicit arrow structure;
5. ordinary Array and Tuple expressions outside contracts are unchanged; and
6. parser recovery retains the Array delimiters and reports a missing item or
   bracket locally.

The parser must not branch on a resolved or textual callee name to decide call
arity. In particular, no `name == "Tuple"` lowering rule remains.

General annotation and `type` initializer slots already accept ordinary
expressions. Their grammar does not need to reinterpret Tuple calls; they only
need the prelude rename from explicit callable `Fn` to `Func` where applicable.

## Static and runtime semantics

The canonical metadata vocabulary changes in one place:

```text
{kind: 'Function, parameters, result}
    ->
{kind: 'Func, parameters, result}
```

Public `TypeDescKind` and erased/Dyn kind observers use `'Func` for callable
values and descriptors. The structural and runtime semantics do not change:

- Function descriptors retain ordered parameter descriptors and one result;
- Tuple descriptors retain a flat ordered list of item descriptors;
- Tuple assignability remains equal-length, position-wise assignability;
- Function checking, rigid generic contracts, inference, variance policy, and
  callable obligations remain unchanged;
- Function and Tuple values keep their existing runtime representation;
- parameterized families substitute into the same normalized descriptors; and
- TypeMetadata observers report `Func` and `Tuple` kinds.

Diagnostic, hover, and semantic displays continue to prefer authored Function
notation and normalized Tuple display:

```text
Fn(Int) -> (String, Bool)
```

The display does not imply that `(String, Bool)` is newly accepted as authored
Tuple type notation. It is the existing normalized descriptor presentation.

## Compatibility and migration

Existing authored Function contracts remain source-compatible:

```telora
Fn(A) -> B
```

Explicit metadata construction and old restricted-contract Tuple spelling
migrate mechanically:

```telora
Fn([A], B)   -> Func([A], B)
Tuple(A, B)  -> Tuple([A, B])
```

Repository native modules, examples, tests, and the language SSOT migrate in
the same implementation commit. The old callable `Fn` binding and the
contract-only `Tuple(A, B)` rewrite are removed rather than retained as
permanent aliases.

Focused diagnostics should recognize likely old spellings:

```text
Fn is Function type notation; write `Fn(A) -> B` or `Func([A], B)`
Tuple metadata takes one Array; write `Tuple([A, B])`
```

No compatibility path may make `Fn(A, B)` mean a one-parameter Function
returning `B`; the missing arrow remains unambiguous.

The source name `Function` is not reserved by this RFC. Existing domain types
with that name remain valid; descriptor matching uses the atom `'Func`.

## Tooling and recovery

The lossless CST retains authored Function notation, nested Tuple calls, Array
delimiters, punctuation, trivia, and missing tokens. Typed syntax views expose
Function parameters/results and complete nested contract items.

HIR and semantic facts may record the elaborated `Func` constructor reference
while retaining the authored `Fn` location. Hover and diagnostics use the
normalized surface display. Completion inside Function parameters/results and
Tuple Array items uses existing TypeMetadata binding facts.

Damaged notation produces local parser diagnostics and must not suppress
independent bindings or fabricate `Any`-based accepted contracts. Recovery may
publish unavailable facts under the existing workspace rules.

## Implementation plan

1. extend CST contract rules and typed views to retain metadata Array arguments
   inside nested contract calls;
2. change Function notation lowering from `Fn([parameters], result)` to
   `Func([parameters], result)` with source-preserving synthetic shells;
3. rename the core callable metadata constructor from `Fn` to `Func` in runtime
   and static prelude environments;
4. rename canonical metadata encoding/decoding and public observer variants
   from `Function` to `Func`;
5. remove the parser's textual `Tuple` special case;
6. migrate native modules and repository sources from `Tuple(A, B)` to
   `Tuple([A, B])` and explicit `Fn([parameters], result)` to `Func(...)`;
7. update `docs/design/LANGUAGE.md` as the language SSOT;
8. add syntax, parser, HIR, strict inference, tool-stage, runtime, recovery,
   module-interface, hover, and display tests; and
9. run formatting, strict Clippy, the full workspace suite, and diff checks.

## Implementation result

Implemented in `telora-core` by extending the restricted contract grammar with
metadata Array arguments and lowering authored `Fn` notation to ordinary
`Func` calls. The previous callee-name-based Tuple wrapping is removed. The
runtime and static preludes expose `Func`, and canonical metadata encoding,
decoding, substitution, validation, `std/type-desc`, and `std/dyn` consistently
use the `'Func` kind.

Standard native contracts and reference interpreters now use
`Tuple([A, B])`. The language SSOT records the `Fn`/`Func` boundary, the single
Tuple constructor protocol, the public observer vocabulary, and the fact that
`Function` remains available to domain models.

Tests cover syntax lowering and rejection, inline `def`, `decl`, and `native`
contracts, nested Tuple metadata, explicit `Func` construction, removed Tuple
rewriting, canonical metadata round trips, both public kind observers, and a
domain type named `Function`. Formatting, strict workspace Clippy, and the full
workspace test suite pass.

## Acceptance criteria

1. `Fn(A) -> Tuple([B, C])` parses and checks inline in `def`, `decl`, and
   `native` contracts without a type alias;
2. `Tuple([A, B])` produces equal canonical metadata in `type`, `let`, closure,
   `def`, `decl`, and `native` positions;
3. `Fn(A) -> B` and `Func([A], B)` produce equal canonical Function metadata;
4. empty, singleton, pair, and three-item explicit Tuple constructors preserve
   exact flat arity and item order;
5. higher-order Function contracts and nested Tuple parameters/results compose
   without aliases;
6. parameterized families preserve rigid parameters through surface Function
   notation and explicit `Func` construction;
7. whole-module, selective, open, and aliased imports preserve the same precise
   schemes;
8. `(Int, String)` in ordinary program code remains a Tuple value containing
   TypeMetadata values;
9. old `Tuple(A, B)`, explicit callable `Fn([A], B)`, malformed arrows, and
   non-Array `Func` parameters produce focused diagnostics;
10. no callee-name-based Tuple argument wrapping remains in parser or
    elaboration code;
11. metadata encoding, `std/type-desc`, Dyn observation, codecs, schemas, and
    reference interpreters consistently expose `'Func`, never `'Function`;
12. an existing user or domain type named `Function` remains legal;
13. Function/Tuple descriptor observation, validation, codecs, schemas,
    interpreters, equality, display, and hashing do not regress; and
14. full workspace tests, formatting, strict Clippy, and diff checks pass.

## Non-goals

- adding `(A, B)` or another new Tuple type notation;
- changing Function variance, generic inference, higher-rank types, callable
  obligations, or closure syntax;
- changing Tuple runtime layout, equality, pattern semantics, or fixed-arity
  checking;
- adding recursive heterogeneous lists, `Cons`, `Nil`, dependent pairs, named
  Tuple fields, or variadic generic parameters;
- replacing ordinary programmable TypeMetadata with a closed type-only
  evaluator;
- changing Array, Dict, Option, Result, Struct, Enum, Union, or user-family
  constructor semantics; or
- retaining legacy spellings indefinitely.

## Rejected and deferred alternatives

### Add `(A, B)` Tuple type notation now

This conflicts with ordinary Tuple values containing first-class TypeMetadata.
Resolving the meaning by recursively reinterpreting expressions in annotations
would change arguments to programmable metadata helpers; resolving it through
type inference would introduce a circular analysis dependency. It is deferred.

### Keep `Tuple(A, B)` as contract sugar

The spelling looks like an ordinary call but violates the real one-argument
Tuple interface and changes meaning by source position.

### Use `Cons(A, B)` for Tuple notation

`Cons` conventionally describes one recursive head/tail cell. Telora Tuple is
flat and variadic, so the name would misstate its structure.

### Name the callable constructor and descriptor `Function`

`Function` is a common domain term and is already used by a repository SQL
model. `Func` matches the existing runtime callable vocabulary, remains visibly
distinct from `Fn`, avoids that source break, and now names the descriptor kind
as well.

### Use only explicit `Func` calls

Requiring `Func([A], B)` everywhere would be uniform but would make common
higher-order contracts harder to scan. The arrow is an explicit, unambiguous
surface boundary and justifies the small notation.

### Keep `Fn` as both syntax and callable value

The dual role saves one identifier but retains contextual tokenization and
documentation ambiguity.

### Make `Func(A, B)` infer parameter/result roles

This would make the first argument inconsistent with the actual Array-based
constructor protocol and would not scale to zero or multiple parameters without
variadic call semantics.

## Stopping rules

Return to design discussion if implementation requires:

1. a second TypeMetadata evaluator rather than ordinary constructor calls;
2. reinterpretation of ordinary Tuple values in annotation or program
   expressions;
3. callee-name-based rewriting for Tuple or user-defined families;
4. changing Function or Tuple descriptor payload structure beyond the accepted
   `'Function` to `'Func` kind rename;
5. weakening strict contracts with `Any` to recover from damaged notation; or
6. retaining two callable names or context-dependent arities for one metadata
   constructor.

Those outcomes exceed the bounded notation and consistency cleanup defined by
this RFC.
