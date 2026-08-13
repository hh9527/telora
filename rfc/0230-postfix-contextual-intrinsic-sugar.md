# RFC 0230: Postfix contextual intrinsic sugar

- Status: Accepted for implementation
- Depends on: RFC 0101

## Summary

Telora accepts a uniform postfix spelling for every contextual intrinsic:

```telora
receiver.ident!(arguments...)
```

It is exact syntax sugar for:

```telora
ident!(receiver, arguments...)
```

The rewrite happens before the named intrinsic's arity and semantic checks.
It adds no method dispatch, member lookup, user-defined macro system, runtime
reflection, or new effect capability.

Examples include:

```telora
value.dbg!()
value.dbg!("validated")
error.raise!()
"OutOfRange".fail!(array, index)
"deprecated".emit_warn!(value)
```

These are respectively equivalent to:

```telora
dbg!(value)
dbg!(value, "validated")
raise!(error)
fail!("OutOfRange", array, index)
emit_warn!("deprecated", value)
```

## Motivation

Contextual intrinsics often operate on the value immediately produced by a
larger expression. Prefix wrapping interrupts the authored data flow:

```telora
dbg!(make_plan(model, request), "generated")
```

The postfix form keeps the receiver adjacent to the operation:

```telora
make_plan(model, request).dbg!("generated")
```

Defining this only for `dbg!` would create an unexplained special syntax.
Defining one argument-prepending rule for the existing closed intrinsic surface
is smaller and makes its behavior predictable.

## Grammar and precedence

Postfix intrinsic application is part of the existing postfix expression
chain, alongside calls, indexing, type application, propagation, and
projection. Its surface grammar is:

```text
postfix_intrinsic := expression '.' Identifier '!' arguments
arguments         := '(' [expression (',' expression)* [',']] ')'
```

It binds more tightly than unary and binary operators and associates with the
postfix chain from left to right:

```telora
request.user.dbg!()
# dbg!(request.user)

make().dbg!().field
# dbg!(make()).field

items[0].dbg!("first")
# dbg!(items[0], "first")
```

The receiver is the complete expression to the left at that postfix-chain
point. Parentheses remain available to choose a wider receiver:

```telora
(left + right).dbg!()
```

`value.ident ! (...)` is governed by ordinary token whitespace rules; the CST
must still recognize the exact `.` `Identifier` `!` `(` sequence rather than
retroactively interpreting a completed field access.

## Lowering

The CST retains a distinct postfix-intrinsic node with receiver, intrinsic
identifier, and explicit argument children. AST lowering constructs the same
contextual intrinsic representation used by the prefix spelling after
prepending the receiver to the explicit arguments.

Conceptually:

```text
postfix(receiver, name, [a, b])
  -> contextual(name, [receiver, a, b])
```

The authored receiver node is preserved, including its exact source location
and source text. Intrinsics such as `dbg!` that capture first-argument context
therefore report the receiver expression rather than the complete postfix
invocation.

No ordinary `Field` node for `ident` is created. HIR name resolution does not
look up `ident` on the receiver, and the receiver's runtime type cannot affect
which intrinsic is chosen.

## Closed intrinsic namespace

Postfix syntax does not open contextual intrinsic names to user code. The same
reserved-name table handles both spellings:

```telora
value.unknown!()
# unknown contextual intrinsic unknown!
```

An intrinsic receives exactly one additional first argument from the postfix
receiver. Its existing arity and type rules then apply. For example:

```telora
error.raise!()       # valid: raise!(error)
error.raise!(other)  # invalid: raise! receives two arguments
```

The rewrite is purely syntactic. It does not grant access to an intrinsic that
is reserved but unavailable, and it does not turn an ordinary function,
decorator, or imported binding into postfix syntax.

## Evaluation

The postfix spelling has exactly the evaluation order and observable semantics
of its prefix expansion. The receiver occupies the first argument position and
is evaluated according to that intrinsic's ordinary rules. The sugar does not
evaluate, copy, retain, or inspect the receiver by itself.

Because the two spellings share one lowered representation, type inference,
expected-type propagation, source provenance, fuel, failure, diagnostics, and
Host observation cannot distinguish them except for authored source ranges
explicitly defined by an intrinsic such as `dbg!`.

## Interaction with pipelines

Postfix intrinsic syntax is independent of reverse application:

```telora
request
|> make_plan(model, _)
|> dbg!(_, "generated")
```

continues to work through the prefix intrinsic and placeholder section rules.
Postfix syntax is useful when the expression is already local:

```telora
make_plan(model, request).dbg!("generated") |> lower_sql
```

This RFC does not add a special pipeline stage or change placeholder lowering.

## Rejected alternatives

### Add only `.dbg!()`

A one-name postfix exception would enlarge the grammar without establishing a
general rule and would make future intrinsic spellings arbitrary.

### Treat it as a method call

Contextual intrinsics need compiler-authored syntax context and are selected
from a closed language namespace. Method lookup would incorrectly make the
receiver type, imports, and local member resolution participate.

### Add user-defined postfix macros

A macro system requires expansion phases, hygiene, source mapping, capability
boundaries, and termination rules. Argument-prepending sugar for reserved
intrinsics does not require or imply such a system.

### Remove the prefix spelling

Prefix syntax remains the canonical grammar and is sometimes clearer for
multiline expressions. Both spellings lower identically and do not create two
semantic capabilities.

## Implementation plan

1. Add a postfix contextual-intrinsic production at the postfix precedence
   level with lossless CST coverage.
2. Lower it by prepending the authored receiver to explicit argument nodes and
   invoking the existing contextual-intrinsic lowering path.
3. Preserve receiver source text and location for contextual consumers.
4. Add parser, precedence, arity, type, provenance, pipeline, and recovery
   tests for both spellings.
5. Document the uniform sugar in the language SSOT and tutorial.

## Acceptance criteria

1. `receiver.ident!(args...)` and `ident!(receiver, args...)` lower to the same
   intrinsic operation and produce the same type and runtime result.
2. Postfix application composes correctly after calls, fields, tuple
   projections, indexes, type applications, and parenthesized expressions.
3. Following field access, calls, indexes, and additional postfix intrinsics
   associate left to right without ambiguity.
4. The receiver is prepended exactly once and preserves its authored source
   location and text.
5. Unknown, unavailable, wrong-arity, and wrong-type intrinsics produce the
   same diagnostics under both spellings.
6. No ordinary field/member lookup or user-defined macro resolution occurs.
7. Existing prefix contextual intrinsics and ordinary projection syntax retain
   their behavior.
