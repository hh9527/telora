# RFC 0232: Bounded recursion for TypeMetadata families

- Status: Implemented
- Tracking issue: #51
- Clarifies: RFC 0218

## Summary

Telora does not support parameterized recursive TypeMetadata families. Every
strongly connected type-declaration component containing a family is rejected.
Recursion remains available through closed, non-parameterized concrete types;
acyclic families may contain those closed recursive descriptors or accept them
as ordinary TypeMetadata arguments.

```telora
@struct type CallExpr = {name: String, args: Array(Expr)};
@enum type Expr = {Literal: Int, Call: CallExpr};

@struct type Dialect(Context) = {
    render: Fn(Context, Expr) -> String,
};

type SqlDialect = Dialect(SqlContext);
```

This is the bounded design decision for #51. It preserves one finite canonical
metadata graph per recursive concrete type and one finite symbolic template per
family, without adding recursive family application nodes, higher-kinded
parameters, or per-instantiation execution.

## Existing metadata models

A family declaration is evaluated once with rigid `Bound` descriptors:

```telora
type Box(A) = {value: A};
```

It publishes a finite symbolic template and the scheme
`for(A) Fn(TypeOf(A)) -> TypeOf(Box(A))`. Applying `Box(Int)` performs
capture-avoiding substitution in that template. It does not execute the body
again.

A closed recursive declaration uses a different protocol. Names in one
recursive component receive graph references before their bodies are sealed;
the completed finite graph is then frozen and published atomically. Recursive
identity belongs to that concrete graph root.

Combining these models is not a local relaxation. In:

```telora
type Expr(Leaf) = Union([Leaf, Array(Expr(Leaf))]);
```

the back-edge is neither a closed concrete name nor an ordinary occurrence of
`Bound(Leaf)`. It denotes a recursive application of the current family under
an argument environment. Supporting it requires a new descriptor node and
rules for instantiation identity, memoization, sealing, substitution, codec and
schema traversal, publication, and resource accounting.

## Accepted boundary

The following are accepted:

1. closed direct or mutual recursion among non-parameterized concrete types;
2. an acyclic family whose template contains an already closed recursive type;
3. a family that receives a closed recursive type as an ordinary argument;
4. concrete aliases applying such families;
5. codec, schema, Dyn observation, and module export over the resulting one
   canonical finite graph; and
6. parameterizing behavior outside the recursive data shape, such as a
   renderer, visitor, capability, policy, or dialect record.

The following are rejected:

1. direct recursive family application;
2. mutual recursion between families;
3. a cycle containing both a family and a concrete declaration;
4. recursion hidden through a local helper needed to build the family template;
5. eager unfolding, depth-bounded approximation, or replacement with `Any`;
6. passing a family itself as a type parameter; and
7. choosing a different family body result for each concrete argument.

## Identity and publication

Closed recursive types retain their existing graph-root identity and sealing
protocol. Families retain structural template identity and capture-avoiding
substitution from RFC 0218. Applying an acyclic family to a recursive concrete
argument embeds or references the already sealed graph; it does not create a
new recursive family identity.

No provisional recursive family, unsealed up-link, free `Bound`, inference
variable, or `Any` approximation may enter MainWorld, a module interface, a
codec/schema graph, completion, hover, or a recovery snapshot.

## Termination and resources

Family dependency scheduling operates over a finite declaration graph.
Evaluation accepts only acyclic family components, evaluates each symbolic body
once, and substitutes finite validated metadata arguments. Recursive concrete
sealing remains separately bounded by existing tool-stage fuel, allocation,
stack, call-depth, cancellation, and publication limits.

The language does not attempt a positivity check, contractiveness proof,
regular-tree unification, recursive type-function normalization, or
instantiation cache for recursive family applications. Adding any of these
would require a new RFC with evidence beyond one ontology expression shape.

## Recommended modelling

Share a closed recursive data algebra when the set of leaves is stable, and
parameterize operations around it:

```telora
@struct type Binary = {left: Expr, right: Expr};
@enum type Expr = {Int: Int, Text: String, Add: Binary};

@struct type Renderer(Context) = {
    render: Fn(Context, Expr) -> String,
};
```

When dialects genuinely require different leaf sets, declare separate closed
recursive types or define one closed enum containing the supported variants.
Use an ordinary tagged extension payload only when dynamic openness is part of
the domain contract; do not use `Any` or `Dyn` to imitate an open recursive
family.

## Rejected alternatives

### Add a recursive family reference descriptor

This is the complete mechanism, but it creates a second recursive identity
model parameterized by an environment. Codec/schema, Dyn, equality,
publication, import, and recovery would all need to agree on its canonical
instantiation graph. The current evidence does not justify that complexity.

### Eagerly unfold applications

Unfolding never reaches a finite descriptor for recursive inputs. A depth limit
changes type identity according to a resource setting and cannot support exact
codec or schema graphs.

### Re-run the body for each concrete application

RFC 0218 deliberately evaluates a family body once. Re-execution could branch
on concrete descriptor structure and disagree with the published generic
scheme.

### Approximate the recursive edge with `Any` or `Dyn`

This removes the static relation that TypeMetadata is intended to preserve and
allows codec/schema or projection failures to move from declaration time to
runtime.

## Acceptance criteria

1. direct, mutual, and mixed recursive components containing a family produce
   stable sourced diagnostics;
2. no rejection path publishes `Any` or a provisional family scheme;
3. a closed recursive type can be captured by and passed through an acyclic
   family without losing recursive identity;
4. the accepted pattern survives module export and drives codec/schema through
   one canonical finite graph;
5. LANGUAGE SSOT and the ontology tutorial state the boundary and workaround;
6. RFC 0218 remains authoritative for ordinary acyclic family templates; and
7. full workspace tests, warning-denied Clippy, formatting, and diff checks
   pass.

## Implementation result

The existing dependency scheduler already rejects every recursive component
containing a parameterized family and reports all cycle participants. Existing
concrete recursion, family substitution, module publication, codec, and schema
paths already implement the accepted boundary. RFC 0232 adds the combined
regression coverage and makes this behavior the explicit design decision rather
than a temporary missing feature.
