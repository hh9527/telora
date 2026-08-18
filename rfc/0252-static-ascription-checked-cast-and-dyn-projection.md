# RFC 0252: Static Ascription, Checked Cast, and Exact Dyn Projection

- Status: Implemented
- Tracking issue: #99
- Depends on: RFC 0052, RFC 0055, RFC 0178, RFC 0248, RFC 0250

## Summary

Telora separates four operations that were previously conflated by symmetric
`Any` assignability:

```telora
ty!(expression, Target)
expression.ty!(Target)

cast!(expression, Target)
expression.cast!(Target)

dyn.project_with(Target, package)
dyn.project@[Target](package)
```

`ty!` is zero-runtime bidirectional static ascription. `cast!` is a
representation-preserving checked refinement and returns
`Result(Target, String)`. Dyn projection compares the package's exact
canonical type identity and returns `Option(Target)`. Codec and translation
remain the only operations that may parse, convert, unwrap a public data sum,
or rebuild a graph.

`Any` becomes directional: every `T` may widen to `Any`, but `Any` never
narrows to `T` through ordinary checking. No new stable public API is based on
`Any`.

## Static ascription

The prefix and postfix forms of `ty!` are the same AST operation. The second
operand must evaluate at tool stage to `TypeOf(Target)`. Type checking sends
`Target` as the expected type into the first operand, then requires the
inferred result to be assignable to `Target`:

```telora
let empty = [].ty!(Array(Int));
let truth = 'True.ty!(Bool);
```

The result type is `Target`. The compiler emits exactly the code for the first
operand: no call, allocation, witness rewrite, source-location rewrite, or
other runtime instruction is permitted. Consequently `ty!` cannot recover a
`Target` from `Any` or `Dyn`; inability to prove the relation is a frontend
error.

This is not an unchecked assertion. It is an explicit expected-type boundary
for bidirectional inference.

## Checked cast

The prefix and postfix forms elaborate to one reserved checked primitive with
the semantic signature:

```telora
for(Target, Source)
Fn(TypeOf(Target), Source) -> Result(Target, String)
```

The operation recursively validates the source representation against the
target descriptor. Success returns the same logical graph and provenance and
installs the exact target nominal witness where required. A raw Dict or Atom
may therefore receive a nominal target when its complete representation
matches:

```telora
let raw = {id: 1, name: "Ada"};
raw.cast!(User);                 // Result(User, String)

let atom: Atom = 'True;
atom.cast!(Bool);                // Result(Bool, String)
```

An already witnessed nominal value is not raw. If its canonical `TypeId`
differs from the target, cast fails even when both declarations have identical
bodies. Widening through `Any` does not remove the runtime witness and cannot
bypass this rule.

Validation errors use stable root-relative paths such as `value.address.zip`.
They are ordinary `String` payloads in `Err`; only an explicit `unwrap!` or
`must_ok!` turns them into failure diagnostics. A Fail operand follows normal
Fail propagation and does not become `Err`.

Cast never parses String, converts Int and Float, unwraps `Value`, applies
codec attributes, changes field names, inserts defaults, flattens structures,
or otherwise rebuilds business data. In particular:

```telora
"1".cast!(Int);       // Err
1.cast!(Float);       // Err
value.cast!(User);    // Err when value is the public tagged Value sum
```

## Exact Dyn projection

`Dyn` is an existential package:

```text
exists T. (TypeOf(T), T)
```

The standard module publishes:

```telora
project_with: for(T) Fn(TypeOf(T), Dyn) -> Option(T);
project:      for(T) Fn(Dyn) -> Option(T);
```

`project_with(Target, package)` succeeds only when the canonical type encoded
by the package descriptor is exactly `Target`. Primitive built-ins use their
canonical built-in IDs; declared types use their full canonical constructor
and argument identity. No structural cast or assignability fallback occurs.
For example, an Atom packaged as Atom cannot project as Bool.

`project@[Target](package)` is dedicated syntax sugar for
`project_with(Target, package)`. The elaborator recognizes the resolved
canonical `std/dyn.project` binding, not the text `project`; a local function
with that name remains an ordinary generic call. This does not introduce a
general implicit-witness rule for explicit type application.

In a generic context, `project@[T]` is accepted only when a runtime
`TypeOf(T)` witness is available. Otherwise it is a frontend error rather than
an erased or structural guess.

## Directional Any

Ordinary assignment, arguments, returns, branch checking, and annotations use
the following direction:

```text
T -> Any   allowed
Any -> T   rejected
```

`Never` retains its bottom behavior. Internal inference variables and recovery
errors remain internal and must not be published as user-visible `Any` merely
to make a narrowing pass. Existing explicitly dynamic observers may consume
`Any`, but new public boundaries must choose `Dyn`, the public tagged `Value`,
or a generic parameter according to their semantics.

## Codec boundary

Codec and translation consume a public data model and construct a different
typed graph. Their behavior includes parsing, numerical conversion where
specified, tagged `Value` removal, attributes such as rename/default/flatten,
and source-to-target provenance mapping. None of these are cast semantics.

The existing `validate(Type, Any)` primitive is a legacy explicit checked
boundary. It does not justify implicit `Any -> T`, and new code should use
`cast!`, exact Dyn projection, or codec according to intent. Its eventual
removal or narrowing is independent of this RFC.

## Runtime and provenance invariants

1. `ty!` emits no runtime operation.
2. Successful cast preserves the source payload graph and source provenance;
   only canonical witness views may be installed.
3. Failed cast allocates only its `Err(String)` result and does not mutate the
   source graph.
4. Dyn projection returns the packaged payload unchanged on success.
5. Fail propagates before either operation constructs `Err` or `None`.
6. Neither operation rebases a value to the macro call location.

## Rejected alternatives

- Symmetric `Any` assignability is unchecked narrowing.
- Making `ty!` an ordinary identity function loses inward expected-type flow
  and emits a runtime call.
- Structural Dyn projection violates its existential type identity.
- Erasing nominal identity before cast lets unrelated declarations intercast.
- General implicit insertion of `TypeOf` arguments changes every generic call
  and creates parameter-count ambiguity.
- Routing `Value -> model` through cast conflates validation with graph
  lowering and codec attributes.

## Implementation plan

1. Add a dedicated ascription AST node and contextual prefix/postfix lowering;
   collect and evaluate its target metadata, infer its operand with the target
   expected type, and compile only the operand.
2. Add a reserved checked-cast core primitive, contextual lowering, exact
   nominal-conflict checks, nested validation paths, and witness-preserving
   success construction.
3. Make `assignable` and inference checks directional for `Any`, then migrate
   intentional dynamic bridges to their explicit checked operations.
4. Add `std/dyn.project_with` and resolved-binding elaboration for
   `project@[T]`; compare canonical runtime identities only.
5. Update the language SSOT and tutorials and audit public signatures for new
   accidental `Any` boundaries.

## Acceptance criteria

1. Prefix and postfix `ty!` are equivalent, guide empty/container and nominal
   literal inference inward, reject unprovable narrowing, and emit no runtime
   operation.
2. `cast!` returns `Result(Target, String)` for primitive and raw structural
   inputs, reports stable nested paths, rejects parse/conversion and distinct
   nominal identities, preserves provenance, and propagates Fail.
3. Ordinary `T -> Any` remains valid and `Any -> T` is rejected.
4. `project_with` and `project@[T]` succeed only for the exact packed canonical
   type, preserve the packaged value, and reject missing generic witnesses.
5. Codec tests continue to prove that conversion and attribute handling are
   not cast behavior.
6. The SSOT and tutorials state the four boundaries without exposing VM meta
   as language semantics.
7. The complete workspace test suite passes.
