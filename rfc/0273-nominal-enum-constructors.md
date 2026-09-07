# RFC 0273: Nominal Enum Constructors

- Status: Accepted
- Tracking: [#160](https://github.com/hh9527/telora/issues/160)
- Branch: `feat/0273-nominal-enum-constructors`
- Baseline: `0982e92`
- Implementation: Complete. Contextual constructors retain complete built-in
  enum contracts; quoted payload constructors accept explicit function contracts.
  Public Atom/Tagged constructors and Atom formatting are removed. Debug build
  and workspace tests pass, including 285 language fixture groups.
  Unresolved-owner obligations, branch evidence, propagation and pattern checks
  are implemented. Static metadata, reflection and codec admission are migrated.
  Final acceptance includes deferred constructor-function context and Dyn
  constructor witnesses. No release binary was built.
  The inventory below describes the baseline.
- Scope: Remove standalone Atom/Tagged static types. Preserve source syntax,
  runtime value representation, bytecode representation and Val provenance.

## Motivation

Telora currently has three overlapping descriptions of sum values: singleton
Atom types, structural Tagged types, and nominal enums. A tag can stand alone,
be called to construct a structural payload value, and subsequently acquire
an enum identity from context. The broad Atom type additionally admits every
unowned unit symbol.

Use enum construction consistently instead. This reduces the number of static
type relationships and ensures enum ownership before execution. It does not
remove runtime variant discrimination or promise a VM performance improvement.

## Language Contract

Remove the public `Atom` type, the `Tagged(tag, payload)` type constructor,
singleton symbol types and standalone structural Tagged types. Do not provide
a compatibility mode or implicit conversion to Dyn or String.

Preserve existing enum declarations, quoted variant expressions, calls and
patterns:

```telora
type Status = enum { 'Ready, 'Failed(String) };
let ready: Status = 'Ready;
let failed: Status = 'Failed("reason");
let explicit = 'Ready.ty!(Status);
```

The variant name must belong to the target enum, and its unit/payload form and
payload contract must match. Distinct nominal enums remain distinct even when
their variants and payloads coincide. Generic enum applications retain their
type arguments. Existing built-in enum families keep their canonical identity
rules; this RFC does not redesign all built-in type identities.

A constructor may acquire its owner from an annotation, a parameter, a return
contract, an enclosing collection or product, an operator contract, or evidence
from the complete generic call. Explicit `.ty!(Ty)` and `func@[Ty]` remain
available. Variant spelling alone does not trigger a search of declarations.
Do not synthesize a fresh enum from branches, array elements or matched tags.

```telora
let unknown = 'Ready;        # Error: no enum owner.
let payload = 'Failed("x"); # Error: no enum owner.
let states: Array(Status) = ['Ready, 'Failed("x")];
```

These errors are determined after the permitted inference scope has collected
its evidence, not immediately upon visiting the literal. Inference must remain
independent of sibling argument order. An already resolved enum value cannot
be rebranded by later context.

### Bool and Built-in Families

Bool operators, conditions and guards provide Bool context. Option, Result and
FoldControl contracts provide their corresponding enum context. Payload evidence
can solve the type arguments of a known enum family, but a tag by itself does
not select a family. An absent variant supplies no evidence for its unused type
argument; existing explicit polymorphic contracts and bottom-type rules must be
distinguished from an arbitrary choice of a missing type argument.

Context-free `'True` and `'False` require context like other variants. Conditions,
guards and Boolean operators supply Bool context. No spelling-based default is
introduced. An explicitly constructed Bool is written `'True.ty!(Bool)`.

### Constructors as Functions

```telora
let some: Fn(Int) -> Option(Int) = 'Some;
let value: Option(Int) = some(1);
```

A quoted tag in function context denotes a unary enum constructor when the
function result has a known enum owner and the parameter matches that variant's
payload. It receives a function type, never a singleton Atom callable type.
A unit variant cannot satisfy a payload constructor contract. Without enough
evidence for the function contract, require an annotation or an explicitly typed
closure. No new keyword or constructor syntax is introduced.

Direct `'Some(1)` construction does not require the callee syntax node to be
published as a standalone runtime Atom value. Compiler-internal constructor
markers may remain, but are not public expression types or dynamic witnesses.

### Patterns and Generalization

Patterns are checked against the scrutinee's enum and bind payloads using that
enum's declaration. Preserve exhaustiveness and impossible-variant diagnostics.
Patterns alone do not invent an enum owner for an unconstrained parameter.

Unresolved constructor obligations cannot be generalized into a public implicit
constraint such as "any enum containing this tag". A function returning an enum
constructor needs enough evidence to establish the enum family, though its
ordinary payload type parameters may remain explicitly quantified.

Every executable constructor must be resolved, including discarded values,
overwritten update fields and nested values in arrays, tuples, dicts or closures.
Checking only exported results would leave unowned values executable.

## Static and Runtime Boundaries

Keep internal Atom/Tagged values, the heap layout, tag matching, native value
builders and current bytecode operations. The VM may continue using them to
represent checked enum values and privileged implementation metadata. Keep the
existing Val rule: new values receive construction locations and copied values
retain their source locations and nested identities.

Static reflection must no longer report standalone Atom or Tagged types.
`std/type-desc.TypeDescKind` describes enums through the existing Enum/Ref
mechanisms and `variants`. Remove its Atom/Tagged alternatives when the static
producers are migrated.

`std/dyn.ValueKind` describes runtime representation and retains Atom/Tagged
alternatives. `dyn.kind`, `tag` and `payload` remain useful for observing enum
storage. `dyn.desc` and packing witnesses describe the enum, not its storage
category. Removing static Atom does not require removing the quoted `'Atom`
variant of the declared ValueKind enum.

Packing must resolve the value's enum owner before producing Dyn. Projection
continues to enforce its explicit target identity. Reflection, metadata parsing,
host admission and exported interfaces must not offer a back door for creating
a source-visible standalone Atom/Tagged contract. Private bootstrap metadata
may retain implementation tags provided they cannot escape these boundaries.

Codec admission and schema production operate on enum contracts. Encoding enum
variants and decoding them preserve their existing external representations;
standalone Atom/Tagged codec targets are removed. Existing DecodeError values
and provenance rules are unchanged.

## Formatting and Attributes

Remove `std/fmt.from_atom`, `impl Display for Atom`, and the prepared Atom
projection path. No blanket enum Display implementation is introduced. Enum
formatting uses a declared Display capability, a display property or explicit
matching to text.

Compiler attribute syntax such as `@property('Type)` retains its spelling.
Syntax-level attribute discriminants must be distinguished from ordinary value
expressions; where an attribute is evaluated as a value, supply an explicit
closed contract. Audit generated property, codec, trait and metadata expressions
as well as user-authored code.

## Implementation Inventory

| Area | Existing implementation | Required work |
| --- | --- | --- |
| Public admission | `types/prelude.rs`, `types/tool.rs` | Remove public names and native type constructors; type private bootstrap contracts. |
| Preliminary inference | `types/expression.rs`, dependency discovery | Represent incomplete constructor evidence without publishing singleton types. |
| Authoritative inference | `types/inference-expression/core.rs`, `inference-context.rs`, `inference-unify.rs` | Collect owner obligations; propagate contextual and sibling evidence; resolve before completion. |
| Generalization and completion | `inference-expression/block.rs`, `inference-utils.rs`, `dependency.rs` | Reject escaping obligations in schemes, local expressions and module interfaces. |
| Type identity and reflection | `types/descriptor.rs`, `types/graph.rs`, `types/metadata.rs`, `type_store.rs` | Separate inference markers/private representation from canonical public types. |
| Native and Dyn | `types/prelude.rs`, `vm/dyn.rs`, `vm/type-desc.rs`, heap builders | Preserve enum witnesses across native return, pack/project and inspection. |
| Formatting | `modules/std/fmt.telora`, formatting native implementation, trait selection | Remove Atom capability and old preparation branch. |
| Bootstrap and derivation | property lowering, model generation, codec metadata | Give evaluated discriminants explicit closed contracts. |
| Frontend consumers | analysis, query output, compiler constructor lowering | Report final enum/function types; retain runtime representation. |
| Acceptance | `tests/language`, CLI tests and native-boundary tests | Migrate old semantic expectations; prefer pure Telora behavior fixtures. |

Temporary constructor evidence should have an explicit lifecycle separate from
a publishable type. Reusing the old singleton descriptor without a completion
barrier is insufficient. Obligations need source locations, variant names,
unit/payload or callable form, payload evidence and an owner variable. Shared
generic evidence must constrain that owner without installing an enum identity
on unrelated existing values. Re-inference must not duplicate runtime evaluation
or lose expression records needed by lowering and diagnostic provenance.

## Acceptance Matrix

- Positive: unit and payload variants in annotations, function arguments,
  returns, arrays, tuples, dict values, struct fields, projection and update.
- Positive: later sibling enum evidence, reversed argument order, nested payload
  evidence, generic enum arguments, equality and collection folds.
- Positive: typed first-class constructors, imported/reexported enum identity,
  recursive contracts, Option/Result propagation and exhaustive matching.
- Negative: unresolved unit/payload constructors, discarded unowned construction,
  wrong owner, wrong tag, wrong payload arity/type and conflicting sibling owners.
- Negative: constructor obligations escaping through a generic function, module
  export, TypeOf, reflection, codec target or Dyn witness.
- Boundary: static reflection reports enum identity; runtime kind retains its
  representation category; native returns and codec round trips preserve owners.
- Provenance: copied payload locations, construction locations, failures and
  single evaluation remain unchanged.
- Diagnostics/query: no published standalone Atom/Tagged types; useful missing
  context diagnostics point to the unresolved constructor.

Run a debug build and `cargo test --workspace`, including language fixtures.
Do not build a release binary. Keep pure Telora tests as the primary behavior
coverage; use Rust tests only for boundaries unavailable to source programs.

## Delivery

1. Commit and push the RFC and inventory; record decisions on issue #160.
2. Implement constructor obligations and contextual completion.
3. Migrate public admission, reflection, stdlib and native/generated contracts.
4. Migrate fixtures and current-state guide/design documentation.
5. Run complete verification and review residual static admission paths.
6. Merge the completed branch into main, push main and close issue #160.

Commit and push each completed stage and report its actual verification state
on the issue. Intermediate branch commits do not imply the feature is complete.
Keep main on the existing complete language until final integration. Historical
RFC bodies remain intact; record relevant supersession in their status areas.

## Implementation Audit

Constructor obligations are separate from public type descriptors. They track
source locations, tag names and optional authored payload expressions; binding
an owner propagates context into those payloads. A quoted tag can also acquire
a function contract from later use, with its result owner resolved through the
same obligations. Generic generalization, completed expression records, binding
contracts and resolved graph publication reject unowned constructor types.

Private representation descriptors and shallow tool-stage constructor evidence
remain available internally. Shallow evidence permits rejecting invalid decorator
arguments before tool evaluation; it is not a publishable singleton type.
The VM layout and its reserved representation identities remain unchanged.
Public metadata decoders, static reflection and codec admission reject standalone
Atom/Tagged. Runtime ValueKind continues to describe Atom/Tagged storage.

All new behavioral acceptance cases are pure Telora. Existing Rust boundary
fixtures received source-contract updates where needed. The final debug build,
workspace tests (269 core, 41 CLI, all remaining suites), 285 language fixture
groups and whitespace checks passed on the implementation branch.
