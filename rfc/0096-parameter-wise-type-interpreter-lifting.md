# RFC 0096: Parameter-wise type interpreter lifting

- Status: Implemented
- Depends on: RFC 0055, RFC 0089 through RFC 0095

## Summary

Forma generalizes contextual `interpreter` lifting from the single equality
shape proved by RFC 0093 to Functions whose interpreted inputs and ordinary
closed inputs may be freely interleaved:

```forma
def my_show:
    for(A) Fn(TypeOf(A)) ->
        Fn(A, Bool) -> Result(String, BlameError) =
    interpreter(show_dyn);
```

Given an explicit scheme, the compiler derives the erased operand ABI one
parameter at a time. A parameter exactly equal to a quantified interpreted
type parameter becomes `Dyn`; a parameter independent of every interpreted
type parameter is preserved unchanged:

```text
for(A, B)
Fn(TypeOf(A), TypeOf(B)) ->
Fn(String, A, Bool, B, A) -> R

erased operand:
Fn(String, Dyn, Bool, Dyn, Dyn) -> R
```

Each direct interpreted value is packed with its corresponding witness. Closed
values are passed through without `Any` or `Dyn` erasure. This is an umbrella
RFC: RFCs 0097 through 0099 specify and validate the compiler boundary before
this RFC becomes Implemented.

## Motivation

RFC 0093 intentionally proved the narrow equality adapter:

```text
for(A) Fn(TypeOf(A)) -> Fn(A, A) -> R
```

That shape established the safety of contextual lifting, but its binary and
single-type restrictions are not fundamental. Show, Hash, validation, diff,
and policy-driven rendering need different arities, more than one interpreted
type, or ordinary control inputs. Requiring a dedicated keyword rule for each
shape would turn `interpreter` into operation-specific machinery.

The generalization remains deliberately less powerful than type-directed code
generation. It constructs one ordinary adapter from an explicit static scheme;
it does not synthesize implementations, derive descriptors, or inspect types
during compilation.

## Phase sequence

The planned sequence is:

1. RFC 0097: preserve `interpreter` as a semantic expression until contextual
   type validation and elaboration;
2. RFC 0098: derive witness mappings and perform parameter-wise lifting; and
3. RFC 0099: implement a user-space `my_show` reference example and validate
   the generalized boundary.

Each child RFC is proposed and implemented independently. RFC 0096 remains
Proposed until all three children have recorded their implementation results.

## Accepted scheme

The accepted outer shape is:

```text
for(T0, ..., Tn)
Fn(TypeOf(T0), ..., TypeOf(Tn)) ->
Fn(P0, ..., Pm) -> R
```

The order of witnesses and interpreted parameters need not correspond by
position; correspondence is established by the `TypeOf(T)` argument. Every
interpreted type parameter has exactly one witness. Repeated direct uses of a
type parameter in `P0 ... Pm` reuse that one witness.

For this phase, every inner parameter has exactly one of two classifications:

1. **interpreted**: `Pi` is exactly one quantified type parameter `Tj`; its
   erased ABI type is `Dyn`, and the adapter passes
   `pack_dyn(witness(Tj), value)`; or
2. **closed**: `Pi` mentions none of the interpreted type parameters; its erased
   ABI type remains exactly `Pi`, and the adapter passes the value unchanged.

The result `R` must mention none of the interpreted type parameters. The erased
operand must check against the compiler-derived Function type; source code does
not declare a second signature or choose which parameters are erased.

Examples include:

```text
for(A) Fn(TypeOf(A)) -> Fn(A) -> R
    => Fn(Dyn) -> R

for(A) Fn(TypeOf(A)) -> Fn(A, Bool) -> String
    => Fn(Dyn, Bool) -> String

for(A, B) Fn(TypeOf(A), TypeOf(B)) -> Fn(String, A, B, A) -> R
    => Fn(String, Dyn, Dyn, Dyn) -> R
```

## Rejected shapes

An inner parameter that contains an interpreted type variable but is not
exactly that variable is rejected:

```text
Fn(Array(A)) -> R
Fn(Option(A)) -> R
Fn(Fn(A) -> Bool) -> R
```

These require descriptor derivation for constructed types, structural value
adaptation, or callback bridging. Results containing an interpreted parameter
are also rejected:

```text
Fn(A) -> A
Fn(A) -> Option(A)
```

Lifting is not a Dyn unpacking mechanism and cannot manufacture or recover an
`A`. Missing witnesses, duplicate witnesses, unused ambiguous quantifiers, and
incomplete or non-Function contracts receive source-level diagnostics.

## Semantic elaboration

The compiler retains the authored `interpreter(operand)` expression through
parsing. In the expected generic definition scheme it builds a witness
environment:

```text
Tj -> outer parameter containing TypeOf(Tj)
```

> 后续修订：本节展开中的 operand 求值时机由
> [RFC 0301](0301-interpreter-ordinary-closures.md) 覆盖；下文保留历史记录。

It classifies every inner parameter, derives the erased operand Function type,
checks the operand against that type, and elaborates an adapter equivalent to:

```forma
fn(witness_a, witness_b) {
    fn(prefix, a, flag, b, again_a) {
        erased(
            prefix,
            pack_dyn(witness_a, a),
            flag,
            pack_dyn(witness_b, b),
            pack_dyn(witness_a, again_a),
        )
    }
}
```

The generated closures, parameters, calls, and packs use existing typed HIR
and runtime operations. The semantic node is required so diagnostics and
tooling reason about the authored construct rather than parser-generated names.
No interpreter opcode or runtime callable is introduced.

## Safety argument

The only erased conversion is the existing trusted construction:

```text
TypeOf(A), A -> Dyn
```

The expected scheme supplies both values, and the type checker proves that the
direct parameter has exactly the witness's `A`. Closed parameters retain their
precise static types. The operand cannot return an interpreted type, and user
code gains no Dyn unpack, unchecked cast, or `TypeDesc -> TypeOf(A)` conversion.

Therefore general arity and multiple witnesses add routing, not a wider trust
boundary. Each accepted adapter is reducible to ordinary statically checked
closures plus invariant-preserving Dyn packs.

## Diagnostics and tooling

Diagnostics name the failed invariant and the authored parameter or result:

- missing or duplicate `TypeOf(T)` witness;
- parameter contains `T` but is not exactly `T`;
- result contains interpreted `T`;
- operand differs from the derived erased ABI; or
- `interpreter` lacks a complete explicit contextual scheme.

Module interfaces, type output, hover, go-to-definition, and diagnostics expose
the authored generic scheme and operand. Generated adapter names and Dyn packs
remain internal. Cancellation and stale analysis cannot publish a partially
elaborated adapter.

## Goals

1. make direct interpreted inputs independent of arity and position;
2. support multiple explicit TypeOf witnesses;
3. preserve closed parameters at their exact static types;
4. mechanically derive and check one erased operand ABI;
5. move interpreter validation from parser shape matching to semantic typing;
6. retain ordinary closure/call runtime behavior; and
7. validate the boundary with a user-space Show-like interpreter.

## Non-goals

- descriptor derivation for `F(A)` or nested interpreted parameters;
- callback adaptation when a callback signature contains `A`;
- returning, reconstructing, or recovering an interpreted value;
- additional returned closure layers or arbitrary higher-rank lifting;
- implicit witnesses, capability lookup, traits, or coherence;
- subtyping, `Any` pass-through erasure, or unchecked casts;
- quote, splice, specialization, or runtime code generation; or
- replacing a future native Show implementation.

## Shared acceptance criteria

1. zero, one, or many direct interpreted parameters derive the expected Dyn
   positions without changing closed positions;
2. multiple type parameters use their unique matching witnesses;
3. repeated direct uses of one type parameter reuse its witness;
4. nested interpreted inputs and interpreted results are rejected statically;
5. missing and duplicate witnesses receive dedicated diagnostics;
6. incompatible erased operands report the derived ABI without generated names;
7. accepted adapters execute through ordinary closures, calls, and Dyn packing;
8. CLI, LSP, and module interfaces publish only the authored scheme;
9. a Forma `my_show` handles representative primitive and structural values;
10. no new VM opcode, interpreter registry, trait mechanism, or code generator is
    added; and
11. full workspace tests and strict static checks pass.

## Stopping rules

Work returns to discussion if a child RFC requires:

1. deriving `TypeOf(F(A))` from `TypeOf(A)`;
2. returning or recovering an `A` from erased execution;
3. bridging a callback whose input or result contains `A`;
4. adding another returned closure layer to the accepted shape;
5. introducing implicit witnesses or implicit capability selection;
6. traits, subtyping, higher-rank inference, or code generation; or
7. operation-specific behavior in the general `interpreter` elaboration.

These are separate language-design problems, not implementation details of
parameter-wise lifting.

## Delivery discipline

Each child RFC receives a proposal commit followed by a distinct implementation
commit containing tests and its implementation-result amendment. The umbrella
status and implementation result are updated only after RFC 0099 demonstrates
the complete phase. No child silently broadens an accepted shape to make its
example easier.

## Implementation result

RFCs 0097 through 0099 complete the parameter-wise lifting phase. Interpreter
syntax is now retained as an authored AST node through HIR and semantic analysis,
while bytecode consumes only its validated ordinary elaboration. Contract
validation moved out of parser acceptance and operates on evaluated
TypeDescriptors.

The adapter supports one or more explicit TypeOf witnesses, direct interpreted
inputs in arbitrary positions, repeated uses of a witness, exact pass-through
inputs, and metadata-only witnesses. It rejects missing and duplicate witnesses,
nested interpreted inputs, callbacks containing interpreted parameters, and
results containing those parameters. Runtime behavior remains ordinary closures,
calls, and invariant-preserving Dyn packs with no interpreter opcode, traits,
lookup, descriptor derivation, or code generation.

`examples/reference-show.forma` validates that the mechanism is not
equality-specific: a unary user-space interpreter recursively renders supported
primitive and structural values and propagates explicit blame for opaque domains.
The existing binary reference equality interpreter remains compatible with the
same generalized path.

Full Forma tests pass with 292 passed and 1 ignored; all 13 CLI and 20 LSP tests
pass, and strict workspace Clippy reports no warnings. The stopping-rule cases
remain deferred rather than hidden behind a broader cast or fallback.
