# RFC 0238: Recursive declared component sealing

- Status: Implemented
- Tracking issue: #85
- Depends on: RFC 0034, RFC 0035, RFC 0157, RFC 0235, RFC 0237

## Summary

Telora extends declared `struct` and `enum` identity to direct and mutually
recursive concrete declarations without changing their authoritative runtime
TypeMetadata model.

```telora
type Node = struct {
    value: Int,
    children: Array(Node),
};

type Expr = enum {
    'Literal(Int),
    'Call(Call),
};

type Call = struct {
    callee: String,
    arguments: Array(Expr),
};
```

Every direct declaration reserves its identity before any body is finalized.
The complete concrete dependency component is evaluated into draft metadata,
published with one forwarding map, rewritten to the reserved declared roots,
and validated before any root becomes observable. Recursive references point
to the declared metadata object, not to an anonymous structural clone.

This RFC does not add parameterized recursion. Canonical applications of
acyclic parameterized families are owned by RFC 0239.

## Motivation

RFC 0237 makes an acyclic declaration an immutable metadata object with a
private identity. Repeating that operation one declaration at a time is not
correct for recursion: a body may refer to a root that has not yet been
published, and copying a completed body before all identities exist can leave
nested edges pointing at anonymous drafts or stale up-links.

Recursive expression algebras are the primary pressure behind declared types.
The implementation must preserve both identity and termination across static
checking, Work/Main movement, tooling, codecs, schema generation, and recovery.

## Component lifecycle

For each concrete dependency component containing direct declarations, the
loader performs these conceptual phases atomically:

1. **Reserve** one deterministic `DeclaredTypeId` for every direct declaration.
2. **Evaluate** all draft metadata using the existing initialized/pending
   recursive links.
3. **Scan** every reachable draft root and build the complete source-to-target
   forwarding map before materializing replacements.
4. **Publish** the component graph into MainWorld once, retaining pre-existing
   MainWorld edges.
5. **Rewrite** every reference to a declaration draft root to its reserved
   declared metadata object. The object's own body edge continues to point at
   its structural definition and is not rewritten into a self-edge.
6. **Validate** root kind, initialized links, graph ownership, and publishable
   type schemes.
7. **Commit** every root together. Any failure discards the complete component.

No partially sealed declaration is visible to ordinary module evaluation,
imports, `show`, LSP queries, codecs, or Host projection.

## Recursive identity

Within a declaration body, a recursive occurrence denotes the same identity
as the declaration root:

```text
Node.children.item === Node
Call.arguments.item === Expr
Expr.Call.payload === Call
```

The equality above is identity equality in the authoritative metadata graph.
It is not display-name equality and not structural equivalence.

Aliases do not reserve additional roots:

```telora
type Node = struct {next: Option(Node)};
type Alias = Node;
```

`Alias` points at the completed `Node` object. Reexports behave identically.

## Static checking and construction

The provisional analysis used while evaluating a recursive component already
knows each reserved declaration identity. A `Named` recursive edge may be used
internally while the draft is pending, but expected-type checking must expose
it as the reserved declared type before checking nested literals.

Consequently this is valid and every nested record receives `Node` ownership:

```telora
let root: Node = {
    value: 1,
    children: [{value: 2, children: []}],
};
```

An early shallow annotation check may recognize a direct declared literal as a
construction site, but the complete bidirectional inference pass remains
responsible for checking every nested field and variant payload. The shallow
pass must not reject valid recursive construction by comparing a structural
draft to an unresolved declaration name.

## Heap and world transfer

Copy collection uses scan/map/materialize ordering for recursive declared
graphs. A forwarding entry is installed before traversing an object's outgoing
edges. This applies to:

- WorkWorld to MainWorld publication;
- WorkWorld to a later WorkWorld relocation;
- declared metadata and ordinary declared values;
- closures, Dyn objects, up-links, and container edges that reach them.

Existing MainWorld edges are retained rather than copied. A copied declared
value points at the canonical target-world owner. Cycles are preserved, shared
subgraphs are copied once, and an invalid foreign or pending edge aborts the
whole operation.

The legacy tree-shaped `Value` boundary cannot represent arbitrary cyclic
ordinary data. It may reject such a projection deterministically, but it must
preserve declared metadata identity and sharing for supported projections and
must not recurse without a visited set.

## Tooling and recovery

`show`, semantic snapshots, hover, completion, and diagnostics display the
authored declared name at the root and may traverse its public body with a
visited set. A repeated identity is rendered as a reference rather than
expanded indefinitely.

Recovery may retain pending or failed component nodes internally. It must not
publish them as successful module exports. Diagnostics identify the authored
declaration and the unresolved or invalid edge; they do not expose private IDs
or heap handles.

## Dyn, TypeDesc, codec, and schema

`Dyn` retains the declared root descriptor and the declared ordinary value.
Structural observers may inspect the public body and return child descriptors,
but root ownership is not erased.

`TypeDesc.kind` and `TypeDesc.children` expose the public Struct/Enum body. They
do not expose the identity key. Traversal terminates through ordinary reference
nodes or a visited identity set.

Codec planning keeps the declared owner while recursively planning its body.
Decode wraps every successfully decoded declared node with the exact owner;
encode requires the same owner before traversing its payload. Schema generation
uses deterministic definitions and references for repeated recursive roots.

## Resource bounds

All graph operations are linear in reachable nodes and edges, modulo existing
deterministic map costs. They use explicit visited/forwarding maps; recursive
metadata depth must not consume the native call stack proportionally.

Fuel and allocation accounting follow the existing tool-stage and heap-copy
contracts. Identity comparison is constant-time in the private declaration key
and does not repeatedly compare nested structural bodies.

## Acceptance criteria

This RFC is complete when:

1. direct self-recursive Struct and Enum declarations load and construct values;
2. mutually recursive Struct/Enum components preserve exact identities;
3. nested expected literals receive the recursive declared owner;
4. aliases, imports, and reexports retain the provider's recursive roots;
5. Work-to-Main and Work-to-Work copies preserve cycles and shared owner edges;
6. Dyn and TypeDesc observers terminate and preserve child descriptors;
7. codec round trips retain identity and recursive schema uses stable refs;
8. failed or incomplete components are never published as successful exports;
9. equal-shaped declarations in separate recursive components remain distinct;
10. focused performance fixtures show bounded graph traversal without repeated
    structural comparison.

## Non-goals

This RFC does not define:

- parameterized recursive families;
- general equi-recursive anonymous types;
- public identity constructors or casts;
- positional Struct/newtype syntax; or
- compatibility with `@struct`, `@enum`, or callable model constructors.
