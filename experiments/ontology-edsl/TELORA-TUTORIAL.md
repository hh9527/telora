# Telora tutorial for ontology eDSL authors

This is the bounded language input for the ontology eDSL experiment. It is not
the complete language specification. When a behavior is not described here,
do not invent a Host capability or bypass the typed Telora model.

Telora is a deterministic, pure, expression-oriented language for compiling
intent into immutable plans. Values are immutable, modules are explicit, and
TypeMetadata is ordinary data evaluated by the same VM as program code.

## Values and bindings

```telora
42                         # Int
3.5                        # Float
"text"                     # String
b"bytes"                   # Bytes
'Ready                     # Atom
'True                      # Bool is a closed Atom type
'Some(1)                   # Tagged value
(1, "one")                # Tuple value
[1, 2, 3]                  # Array
{name: "Ada", active: 'True} # record/Dict value
```

Bool values are `'True` and `'False`; Telora has no truthiness conversion.

```telora
let answer = 40 + 2;
def increment = fn(value) { value + 1 };
```

`let`, `def`, `type`, `for`, `fn`, `match`, `native`, `decl`, `import`, and
`export` are reserved language words. `_` is a pattern or explicit-type-argument
placeholder; closure parameters use named identifiers.

## Operators and control flow

The comparison operators are:

```telora
left == right
left != right
left < right
left > right
left <= right
left >= right
```

Equality and inequality retain structural equality for ordinary compound
values. Ordered comparison accepts only matching `Int`, `Float`, or `String`
operands; there is no mixed numeric coercion. String order is lexicographic over
the exact internal UTF-8 byte sequence, without normalization, locale rules,
case folding, or natural-number sorting.

Prefix `!` returns the opposite canonical Bool for Bool and bitwise complement
for Int. Binary `&`, `^`, and `|` accept only Int. Their precedence is
arithmetic, `&`, `^`, `|`, comparison, `&&`, then `||`, from tighter to looser.
All six comparisons share one non-associative precedence level. Write
parentheses when comparing a comparison result; `a < b <= c` is not a chained
comparison. `&&` and `||` accept Bool and short-circuit. An `if` expression
always has an `else` branch.

## Functions and contracts

```telora
fn(value) { value + 1 }

def identity: for(A) Fn(A) -> A = fn(value) { value };
def map_pair: Fn(Int, String) -> Tuple([String, Int]) =
    fn(number, text) { (text, number) };
```

Function type notation is authored as `Fn(P1, ..., Pn) -> R`. The ordinary
TypeMetadata constructor call is `Func([P1, ..., Pn], R)`:

```telora
type Unary = Func([Int], String);
```

`Fn(Int) -> String` and `Func([Int], String)` produce the same canonical
function metadata.

Generic calls infer type arguments unless an explicit `@[...]` application is
written:

```telora
identity@[Int](1)
pair@[Int, _](1, "text")
```

`_` leaves that type argument to the complete call context. Unmarked
`value[index]` is only Array indexing.

A local annotation inside a generic body cannot refer to type parameters
introduced by the enclosing module-level `for` contract. Put the complete
generic contract on a module-level helper, and do not erase the type with `Any`
to bypass that boundary.

Telora merges branch evidence before widening the result: an `if` inside generic
code that contributes different narrow variants of the same expected enum result
joins them first, then widens to the enum. For example, a typed
`Array(Option(Output))` fold can push `'None` in one branch and `'Some(output)`
in the other. A named helper with a complete contract remains useful when a
callback is otherwise underconstrained.

## Structs, enums, and patterns

```telora
@enum type Entity = {
    Ticket: 'None,
    Agent: 'None,
};

@struct type Requirement = {
    target: Entity,
    reason: String,
};
```

Structs and enums are closed. Fields use `.field`; enum values use Atom or
Tagged syntax.

```telora
match result {
    'Some(value) => value,
    'None => fallback,
}

match pair {
    (left, right) => left,
}
```

Matches over known closed variants must be exhaustive or contain a catch-all.
`_` is the wildcard pattern.

## Arrays

Import the standard Array module explicitly:

```telora
import "std/array" as array;
```

The main operations used by this experiment are:

```telora
array.length(values)                 # Int
values[index]                        # A; fails with OutOfRange blame
array.get(values, index)             # Option(A)
array.enumerate(values)              # Array(Tuple([Int, A]))
array.find(values, predicate)        # Option(A)
array.any(values, predicate)         # Bool
array.all(values, predicate)         # Bool
array.map(values, mapper)            # Array(B)
array.flat_map(values, mapper)       # Array(B)
array.filter(values, predicate)      # Array(A)
array.fold(values, initial, folder)  # State
array.concat([left, right])          # Array(A)
array.push(values, item)             # new Array(A)
```

Arrays preserve order. `find` returns the first matching item, `filter`
preserves input order, and folds process items from left to right. These order
properties may be part of a deterministic eDSL contract.

Both indexing forms use a zero-based Int index. `values[index]` returns the
element directly and a negative or out-of-range index fails with
`fail!("OutOfRange", values, index)`. `array.get` returns `'None` for the same
missing positions. `enumerate` pairs each item with its zero-based Int index
while preserving source order and duplicates:

```telora
array.get(["a", "b"], 1)       # 'Some("b")
["a", "b"][1]                 # "b"
array.enumerate(["a", "b"])   # [(0, "a"), (1, "b")]
```

Use `array.get` when absence is expected control flow and direct indexing when
absence violates an invariant. Use `array.enumerate(values)` when an algorithm
must retain positions.

Build any required bounded structure, such as a graph or set, from typed
immutable values and ordinary library functions.

## Tuple metadata

Tuple values and Tuple TypeMetadata are distinct uses of ordinary values:

```telora
let pair: Tuple([Int, String]) = (1, "one");
let number: Int = pair.0;
type Pair = Tuple([Int, String]);
```

`Tuple` takes exactly one argument, an Array of TypeMetadata: `Tuple([A, B])`.
Tuple values use a non-negative literal projection such as `pair.0`; known
out-of-range positions are analysis errors. Nested forms such as
`Fn(A) -> Array(Tuple([B, C]))` are valid.

## TypeMetadata families

Types are first-class metadata values. `Type` is the type of arbitrary valid
TypeMetadata; `TypeOf(A)` is precise evidence for metadata describing `A`.

Parameterized declarations define reusable TypeMetadata families:

```telora
@struct
type Capability(Id, Input, Output) = {
    id: Id,
    lower: Fn(Id, Input) -> Option(Output),
};

type TicketCapability = Capability(TicketId, Request, TicketPlan);
```

The family is also an ordinary typed metadata capability in value position.
Families must receive all parameters, are rank-1, and cannot be higher-kinded.
Use model-supplied concrete types for identities, payloads, mappings, and
plans. Do not replace unknown relationships with `Any`, `Dyn`, or String
identity.

## Strings and diagnostics

Ordinary strings do not interpolate. Interpolation uses backticks:

```telora
let message = `missing capability \{name}`;
```

Interpolation supports stable scalar representations such as String, Int, and
Atom. It does not render arbitrary Tagged, Struct, Array, Dict, Tuple, Dyn, or
user values. Keep the message static when the subject is structured.

```telora
let error = blame!("missing capability", authored_subject);
let reported = report('Error, error);
let warning = emit_warn!("fallback policy used", authored_subject);
let ignored = emit_error!("missing capability", authored_subject);
raise!(error)
```

- `blame!` constructs a sourced `BlameError` value without reporting it.
- `report` publishes a diagnostic and returns the same error.
- `emit_info!` and `emit_warn!` report non-blocking diagnostics.
- `emit_error!` is reporting convenience; an Error diagnostic makes strict
  module execution fail at its publication boundary even if local evaluation
  continues far enough to collect independent diagnostics.
- `raise!` immediately exits the nearest function with `Never`.

Expected domain rejection must remain distinguishable from an accidental
runtime failure. If the eDSL contract requires a value-level rejected result,
return `Option`, `Result`, an explicit enum, or diagnostic values; do not use
`emit_error!` as a substitute for that result channel.

## Modules

```telora
import "std/array" as array;
import "@src/local.telora" { compile };
import "ontology-lib/types.telora" as types;

export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Requirement, compile };
```

Imports are static. A module exposes only explicit exports. The eDSL must
export every type and function promised to enterprise authors.

An entry under `a2/bin-src/` is a Host-selected root entry, not a source module
located beside `a2/src/`. Import reusable code from that entry with an explicit
source-root path:

```telora
# a2/bin-src/main.telora
import "@src/ontology.telora" { compile };
```

In a `bin-src/` entry, `./ontology.telora` and other `./` or `../` imports are
invalid. Within a module under `a2/src/`, relative imports remain valid and
resolve from the importing module's logical directory. `@src/` always resolves
from the source root of the importing module's own crate. A package path such
as `ontology-lib/types.telora` selects a dependency fixed by the crate manifest;
`std/...` selects a Host-registered builtin module.

## Recursion and bounded work

Telora supports recursive functions with explicit contracts. Calls and
back-edges consume fuel; allocation is also quota-bound. A domain algorithm
must expose any semantic depth bound required by its contract rather than
depending on eventual Host fuel exhaustion.

## Working rules

- Preserve precise generic types through selectors and callbacks.
- Prefer a small named helper with an explicit contract when a nested callback
  is underconstrained.
- Keep enterprise facts and physical mappings outside the reusable eDSL.
- Do not use existing repository ontology implementations as references.
- Do not add Host functions, native declarations, `Any`, or `Dyn` to bypass a
  difficult generic relationship.

Read `a1/EDSL-DESIGN.md` next. It defines the observable eDSL behavior while
leaving module layout, APIs, helper decomposition, and algorithms to you.
