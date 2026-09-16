# RFC 0093: Contextual `interpreter` lifting

> 后续修订：operand 求值时机由 [RFC 0301](0301-interpreter-ordinary-closures.md)
> 覆盖；本文件保留历史设计记录。

- Status: Implemented
- Depends on: RFC 0089 through RFC 0092
- Tracking issue: https://github.com/hh9527/forma/issues/4

## Summary

Forma adds the contextual expression keyword:

```forma
interpreter(expression)
```

In this phase it is accepted as the initializer of an explicitly contracted
`def` with exactly this scheme shape:

```text
for(A) Fn(TypeOf(A)) -> Fn(A, A) -> R
```

where `R` does not contain `A`. The operand must check as:

```text
Fn(Dyn, Dyn) -> R
```

The keyword lowers to ordinary closures and safe `Dyn` packing. It is not a
runtime Function, does not perform lookup, and generates no specialized code.

## Expansion

Conceptually:

```forma
interpreter(erased)
```

expands to:

```forma
fn(type_witness) {
    fn(left, right) {
        erased(
            pack_dyn(type_witness, left),
            pack_dyn(type_witness, right),
        )
    }
}
```

`pack_dyn` is a compiler-owned prelude binding with the same safe contract as
`@bim/std/dyn.pack`:

```forma
for(A) Fn(TypeOf(A), A) -> Dyn
```

It is an implementation hook, not a source-level keyword or dynamic cast.

The expansion occurs before HIR resolution. Generated parameter names are
unspellable by source code, so capture and shadowing follow ordinary closure
rules without introducing user-visible bindings. The existing bidirectional
checker validates the expansion against the complete expected scheme.

## Context restriction

The parser recognizes `interpreter` only as the dedicated call-like expression
form. The type checker accepts the expansion only when an explicit generic
`def` contract provides all of:

- one type parameter `A`;
- one outer parameter `TypeOf(A)`;
- one returned binary Function with both parameters exactly `A`; and
- a result `R` independent of `A`.

Because the expansion itself is checked, an absent or incompatible context
fails through ordinary closure/function diagnostics. This RFC also requires a
dedicated diagnostic naming the accepted interpreter shape rather than exposing
generated parameter names when the outer contract is wrong.

## Why binary-only

Equality is the first validation slice and needs exactly two values of the
same `A`. Supporting arbitrary `P0(A) ... Pn(A)` would require deriving runtime
metadata for constructed parameter types, while zero/one/many direct `A`
positions are only an arity generalization.

This phase deliberately proves one useful shape. Unary Show/Hash and generalized
direct-`A` arity may follow after Eq validates diagnostics and tooling.

## Runtime and tooling

After lowering, bytecode contains only ordinary closures, calls, and the same
native Dyn pack operation already exposed by RFC 0091. There is no interpreter
opcode or runtime binding.

Analysis, module interfaces, CLI type output, and LSP hover publish only the
authored generic contract and ordinary expression facts. Generated names and
the erased adapter expansion are implementation details.

Cancellation, stale revisions, quota accounting, closure capture, tail calls,
and publication use existing mechanisms.

## Goals

1. express a trusted typed boundary without user-authored casts;
2. mechanically connect one `TypeOf(A)` witness to both `A` inputs;
3. check the erased interpreter as an ordinary Forma Function;
4. lower to existing closure/call/runtime machinery;
5. keep authored generic contracts authoritative for tooling; and
6. provide the exact adapter required by RFC 0094 equality.

## Non-goals

- unary, nullary, variadic, or heterogeneous lifted Functions;
- returned values containing `A`;
- callback inputs containing `A`;
- multiple witnesses or type parameters;
- use without a complete explicit contract;
- runtime code generation or specialization;
- recursive adapter dispatch, fallback, or memoization; or
- a general cast or unsafe escape hatch.

## Acceptance criteria

1. a matching `Fn(Dyn, Dyn) -> R` operand lifts to the authored generic scheme;
2. the lifted capability accepts two values of the instantiated `A`;
3. mismatched operand arity, Dyn parameters, or result is rejected statically;
4. missing, monomorphic, unary, returned-`A`, and multi-witness contracts fail;
5. generated names cannot be captured or referenced by source;
6. explicit `eq_fn[Int](Int)` executes through ordinary closures;
7. `eq_fn(Int)` either infers `A` or retains the documented RFC 0055 gap;
8. bytecode adds no opcode and VM adds no interpreter callable;
9. CLI/LSP/module interfaces expose the authored scheme; and
10. cancellation, recovery, quotas, and publication do not regress.

## Implementation plan

1. reserve and parse the `interpreter` keyword expression;
2. lower it to hygienic ordinary closure/call AST;
3. install the compiler-owned safe generic pack binding;
4. add a contextual shape audit and dedicated diagnostics where ordinary
   expansion errors are insufficient;
5. test syntax, typing, execution, interfaces, CLI/LSP, and cancellation; and
6. run the full quality gate and record the implementation result.

## Implementation result

Implemented `interpreter(expression)` as a reserved, lossless CST expression
and a hygienic parser-level expansion. The expression is accepted only as the
direct initializer of an explicitly contracted `def`; lowering audits the exact
single-parameter scheme, witness, binary consumer, and result independence
before introducing ordinary closures and calls. Invalid contexts receive a
dedicated source-level diagnostic and never expose generated names.

The compiler-owned prelude supplies Dyn packing through the existing
`CoreDynFunction::Pack` implementation. No interpreter runtime Function,
bytecode operation, dispatcher, or specialization path was added. The erased
operand remains an ordinary `Fn(Dyn, Dyn) -> R`, so arity and result mismatches
use existing static Function diagnostics.

Regression coverage includes lossless/recoverable syntax, execution, operand
arity rejection, missing and malformed contracts, returned-`A` rejection, and
non-definition use. Both `eq_fn[Int](Int)(...)` and `eq_fn(Int)(...)` execute;
the latter infers `A` from the TypeOf witness and subsequent arguments, so the
RFC 0055 gap does not remain for this shape. Full Forma tests pass with 286
passed and 1 ignored; all 13 CLI and 20 LSP tests pass, and strict workspace
Clippy reports no warnings.
