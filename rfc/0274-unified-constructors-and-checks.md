# RFC 0274: Unified Constructors

- Status: Implemented and merged into main. Construction checks and
  `Unchecked(T)` are carried by [RFC 0275](0275-construction-checks-and-unchecked.md),
  under the same tracking issue; that follow-up remains a draft.
- Tracking: [#161](https://github.com/hh9527/telora/issues/161)
- Branch: `feat/0161-unified-constructors`
- Baseline: `83bb8a6`
- Related: RFC 0236, RFC 0237, RFC 0239, RFC 0270, RFC 0273.
- Delivery: staged commits and pushes, with progress on #161. Completion does
  not authorize merging into main; integration is a separate decision.
- Implementation: newtype declaration metadata, canonical identity, positional
  `.0` access, payload codecs, schema and Dyn tuple observation are implemented
  on the branch. Runtime callable constructors, generic application and
  first-class use and declaration-resolved newtype patterns are implemented.
  Tool-stage callable construction shares strict inference evidence, including
  generic/imported constructors, helper functions, codec ownership and decorator
  arguments. Qualified enum value constructors, including generic members and
  first-class use, are implemented. Module namespaces and selected values now
  carry explicit binding provenance, including same-name module/type aliases.
  Qualified enum patterns validate nominal ownership before nested payload and
  exhaustiveness analysis, including tool-stage patterns. Selective member
  imports/exports retain declaration origin and the complete generic contract
  through module imports and reexports. Imported payload member patterns work.
  Bare unit member patterns resolve imported declaration origin in match, if-let
  and let-else, including tool-stage and cross-module use. Prelude explicitly
  exports Bool, Option and Result members; standard-library bootstrap preserves
  their contracts and provenance, and authored imports take precedence over
  prelude fallbacks. Property markers accept typed tool-stage PropertyTarget
  expressions, including aliases and computed values, with nominal validation.
  Standard-library sources and examples use named enum members throughout.
  Named member payloads receive late nominal context through the shared call
  inference path, including nested builtin and declared generic enums. Existing
  payload values retain their identities. Core behavioral fixtures use named
  members, and providers explicitly export constructors of private enums.
  Language fixture sources and the fixture generator now use named members.
  Branch joins combine enum payload and same-family generic argument evidence;
  return boundaries and collection joins share this propagation. Concrete
  evidence propagates until no further variables can be solved.
  Rust-embedded Telora sources now use named constructors and patterns throughout
  core and CLI tests, while runtime display snapshots retain Atom/Tagged output.
  The lexer, grammar and parser now accept named enum members only in authored
  declarations, expressions and patterns. Compiler-generated Atom/Tagged nodes
  remain private implementation representations. Guides and design documents
  describe named members and family parameter inference, including qualified
  actor, Value and ScalarValue examples. Match coverage, enum payload and
  standard-library diagnostics use current member spellings. Typed tool-stage
  evaluation preserves constructor inference failures as source diagnostics;
  incremental type initialization can still defer incomplete evidence.
  Construction checks are specified separately in RFC 0275.
  Wildcard member selectors are deferred; this delivery uses explicit member lists.
- Validation: PropertyTarget passed debug build, workspace tests and 333 language fixture groups,
  including computed markers, aliases, nominal rejection and marker arity.
  The standard-library syntax migration passed debug build and workspace tests;
  migrated application, reflection and analytics examples passed module checking.
  Named-payload contextualization and migrated core fixtures passed workspace
  tests and all 333 language fixture groups, with three additional `.telora`
  regressions for nested, declared-generic and existing-value payloads.
  Enum branch/return/collection inference and the complete language-fixture
  migration passed debug build, workspace tests and 338 language fixture groups.
  The focused branch suite passed nine cases, including if-let and empty spreads;
  export queries confirm Result(String, Int) for both match-arm orders.
  The Rust-embedded source migration passed workspace tests, including core,
  CLI and language acceptance tests. Diagnostic provenance assertions continue
  to verify both original data and authored failure locations.
  Quoted-syntax removal passed debug build, workspace tests, 269 core tests and 341 language
  fixture groups, including rejection of quoted declarations, values and patterns.
  Eight extracted documentation examples passed module checking; local binding
  snippets were exposed as module definitions for that check.
  Typed tool-stage inference diagnostics passed debug build, 269 core tests,
  342 language fixture groups and workspace tests. An unknown enum member in
  a decorator now reports its static member error before VM evaluation.
  New behavior is tested in
  `.telora`; no release binary was built.

## Objective

Make newtype and enum payload constructors declaration-provided functions.
Resolve constructor identity through names, and infer generic arguments through
ordinary contextual inference. Declaration-bound construction checks are
specified in RFC 0275 and continue to be tracked by #161.

Implement in this order:

1. Newtype declarations, construction, first-class constructors and patterns.
2. Named enum constructors, member exports and removal of quoted tag syntax.

Each stage has its own acceptance gate. The original third stage,
`Unchecked(T)` and `@check(func)`, has moved to RFC 0275 so that its design,
implementation and acceptance can be reviewed independently.

## Existing Implementation

`syntax/telora/grammar.llw` accepts only named fields after `struct` and quoted
variant names after `enum`. `parser/patterns.rs::declared_type_initializer`
lowers declarations to private model operations over member dictionaries.
`parser/bindings.rs` records Struct/Enum declaration kinds separately from
ordinary type expressions.

`types/dependency.rs` reserves nominal declaration identities before evaluating
metadata. Types are tool-stage values; generic type families are callable and
produce `TypeOf` evidence. A newtype constructor cannot simply replace the
existing binding without breaking type application and metadata evaluation.

`TypeDescriptor::Declared` carries a declaration identity and a representation
body. The newtype path must retain identity through generic substitution,
publication, Dyn witnesses and runtime value construction. It must not lower
to an alias of its payload or masquerade as a named-field struct.

The grammar currently supports named export lists, not enum member exports.
The standard prelude is also supplemented by core prelude machinery; updating
only `modules/std/prelude.telora` will not establish all constructor bindings.

Runtime codec handling has separate Struct and Enum kinds and declared-value
wrapping. Newtype support needs an explicit metadata/codec contract, not only
a parser and type-checker change.

## Stage One: Newtypes

```telora
type UserId = struct(Int);
type Box(T) = struct(T);

let id = UserId(1);
let make: Fn(Int) -> UserId = UserId;
let boxed: Box(Int) = Box(1);
```

A newtype is a single-element named tuple, with exactly one payload type and
a distinct nominal identity. `value.0` reads its payload and preserves that
payload's type and source location. Other indices are rejected statically.
General multi-element named tuples are outside this implementation scope.
Wrapping and unwrapping are explicit. Neither the payload nor another newtype
with an identical payload is implicitly assignable to it. A tuple payload can
express multiple components without introducing positional multi-field structs.

Proposed pattern syntax is `UserId(value)`. This is an irrefutable pattern for
a known UserId. Ordinary functions returning UserId are not pattern
constructors. Pattern resolution retains the declaring identity independently
of the first-class function representation.

### Type and Value Names

A declaration supplies both type identity and constructor identity. Type
contracts resolve `Box(Int)` as a type application; value expressions resolve
`Box(1)` as construction. Constructor generic specialization uses existing
`@[Ty]` syntax.

Explicit Type expectations select the type facet, including reflection
arguments and functions whose result contract is Type. An unconstrained bare
newtype declaration name in a value binding denotes its constructor. HIR and
module interfaces retain declaration identity; arbitrary Type-valued bindings
and ordinary functions returning Type do not acquire a constructor facet.
Synthesized module exports preserve declarations and their generic contracts.
Runtime selection does not inspect argument values or constructor spelling.
Tool-stage construction propagates the same inference evidence to its expression
compiler. Incremental metadata evaluation uses currently resolved declarations
and contracts; final inference remains the authority for the complete program.

Imports, exports and aliases must preserve the two facets of the declaration;
they must not manufacture duplicate nominal identities. Duplicate source names
remain errors rather than a way to independently replace one facet.

### Representation and Integration

Preserve the established declared-value identity mechanism and Val provenance.
Payload references retain their source positions; construction records the
new value's construction position. Add explicit newtype metadata so reflection
can distinguish a newtype from its payload and from a named-field struct.

Proposed codec behavior is transparent payload encoding/decoding with nominal
wrapping on successful decode. This needs verification against schema and
decorator contracts before acceptance. Do not automatically inherit payload
traits such as Display or equality solely from representation equivalence.

Named-field projection and struct merge-update remain operations on named
fields; a newtype exposes its positional member `.0`, not a synthetic named
field. Its runtime container holds the original payload Val separately from
the outer declared identity, including when the payload is itself nominal.

### Acceptance Gate

- Distinct newtypes, generic instances, recursive metadata and module aliases
  preserve identity.
- Direct and first-class construction, explicit specialization and contextual
  generic inference agree.
- Patterns unwrap the matching declared type and reject unrelated constructors.
- Type-valued uses remain available alongside value constructors.
- Codec, schema, static reflection and Dyn witnesses describe the same type.
- Negative cases cover implicit wrapping/unwrapping and unresolved generics.

## Stage Two: Enum Constructor Names

```telora
type Event = enum { Progress(Int), Finished };

export Option.{Some, None};
export Result.{Ok, Err};

let result = Ok(1);
```

Payload constructors are functions; unit variants are values. A resolved name
determines the enum family. Context supplies generic arguments, including
arguments absent from a variant's payload; unresolved arguments still produce
an error when no permitted evidence determines them.

This replaces RFC 0273's deferred owner selection for authored constructors.
Do not search all enums by tag spelling or let a return annotation select a
different declaration for an already resolved name.

Members are qualified by the declaring type: `Event.Progress`, including
module-qualified forms such as `events.Event.Progress`. An enum declaration
does not introduce its members as unqualified bindings. A metadata variable
holding the same Type value does not acquire member-constructor identity.

Wildcard member selectors are deferred from this delivery. The module graph
requires a discoverable export list before initializing source modules, so
providers list their public members explicitly, such as
`export Result.{Ok, Err};`. Consumers use the existing module import selectors.

A future `import Event.*;` registers the members as name-resolution candidates. It does
not expand to eager import/definition bindings or by itself load or initialize
the provider module. Actual references select candidates and retain their
declaration origin. Apply the same candidate model to wildcard member exports;
the public member candidates must remain discoverable without evaluating all
member values. Selective member import uses
`import Event.{Progress, Finished as Done};`. Existing
module imports can import constructor names exported by another module.
`export Event.{Progress, Finished as Done};` introduces local member bindings
and exports them. Repeated or conflicting explicit local/public names are
errors, including collisions with other enum families. Wildcard candidates
follow name resolution rather than introducing all of those explicit bindings.
Resolution
retains the declaring family, member name and generic contract through imports
and reexports; ordinary function aliases retain their function contract only.

Generic member specialization uses `Option.Some@[Int]` and
`Result.Ok@[Int, String]`. All family parameters remain part of that contract,
including parameters absent from the selected payload. Contextual inference
may determine them; an unresolved parameter is an error.

Prelude member names already determine Bool, Option or Result ownership. Joining
members of the same family combines generic parameter evidence: both
`if True { Some(1) } else { None }` and the reversed branch order infer
Option(Int). `match Some("hi") { Some(x) => Ok(x), None => Err(2) }` infers
Result(String, Int) without an annotation. Apply this rule to if-let, explicit
return boundaries, collection elements and declared generic enum families as
well. Propagation may require multiple passes when completing an enum supplies
the element type of an empty collection. Different declarations remain distinct,
conflicting concrete parameters are rejected, and missing phantom parameters
still require evidence.

Prelude exports explicitly supply `Bool.{True, False}`, `Option.{Some, None}`
and `Result.{Ok, Err}`. FoldControl remains
available as a type; its members use qualification or explicit member imports.
Codec naming/configuration enums retain their own module/type namespaces.
The intrinsic `@property` target categories use a bootstrap `PropertyTarget`
enum with Type, StructType, EnumType, Member, Field and Variant members, referenced as
`PropertyTarget.Type` and so on. These names are not added unqualified to the
prelude. Bootstrap code uses the same identities as user modules without
relying on the prelude to load itself.

PropertyTarget has a reserved nominal constructor identity, independent of the
source module graph. Capability arguments are type-checked against that identity
and evaluated in the tool stage, so aliases and computed PropertyTarget values
are valid but same-shaped user enums are not capability values. Member preserves
the existing combined field/variant capability.

Implicit prelude candidates are fallbacks: authored bindings and authored
module imports take precedence. Conflicting candidates from multiple authored
open imports remain ambiguous. Bootstrap installation preserves each selected
prelude member's generic contract and constructor provenance along with its value.

The member-binding implementation distinguishes a module namespace from a
selectively imported type even when the module alias and an exported type have
the same spelling. A singleton export lookup is insufficient evidence of a
type binding. `ModuleInterface.value_binding` records the selected value's name;
namespace interfaces retain their full export table. Qualification, reexport,
open imports and standalone direct-value roots preserve this provenance.

Remove quoted forms in declarations, expressions and patterns together.
Migrate generated AST paths, embedded sources, language fixtures, examples,
guides and editor grammar. Underlying Atom/Tagged VM storage is outside the
surface-syntax removal scope.

Acceptance includes conflicting member names, qualified references, module
cycles, prelude bootstrap, higher-order constructors, unit variants, patterns
and complete-call generic evidence. Ordinary function aliases do not become
pattern constructors merely because their return types are enums.

## Construction Checks Follow-Up

RFC 0275 carries `@check(func)` and `Unchecked(T)`, including all previously
listed construction boundaries, provenance guarantees and open design questions.
Completing this RFC does not close #161; that issue also tracks RFC 0275.

## Delivery and Verification

Use focused `.telora` positive and negative fixtures for new behavior. Change
Rust tests only where host-level behavior requires them or existing embedded
source must migrate. Keep parser/editor grammar and diagnostics synchronized.

Run debug builds and appropriate tests for each implementation stage; complete
workspace verification before reporting implementation complete. Do not build
a release binary. Documentation outside RFCs describes supported current
behavior positively. Historical RFC amendments belong in their status areas.

Commit and push reviewable batches to the tracking branch and post evidence,
remaining work and decisions to #161 at stage boundaries. Keep the issue open
until implementation and acceptance are complete. Do not merge into main as
part of this authorization.
