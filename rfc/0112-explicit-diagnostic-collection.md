# RFC 0112: Explicit diagnostic collection

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Privileged capture returns typed Diagnostic snapshots; access remains restricted.
- Depends on: RFC 0056, RFC 0095, RFC 0102, RFC 0105, RFC 0108

## Summary

Forma demonstrates multi-error validation by threading an ordinary persistent
Array of canonical BlameError records:

```forma
type DiagnosticRecord = BlameError;
type Diagnostics = Array(DiagnosticRecord);

def check_user:
    Fn(User, Diagnostics) -> Tuple([User, Diagnostics]) = ...;
```

There is no diagnostic effect, hidden accumulator, dynamic handler, or Host
publication in this phase. A caller receives the completed Array and decides
whether to format, reject, retain, or publish its elements.

## Record model

DiagnosticRecord is an alias for the existing canonical BlameError shape:

```forma
@struct type BlameError = {
    message: String,
    data: Any,
    rule: Any,
};
```

The alias gives collection-oriented code useful vocabulary without creating a
second structurally identical error contract. Records are constructed with
`blame!(data, message)`: `data` retains the blamed value's provenance and
`rule` retains the complete authored intrinsic site.

BlameError does not imply fail-fast control flow. `Result(A, BlameError)` is
appropriate when a computation cannot produce `A`; `Array(BlameError)` is
appropriate when validation can preserve its input and report independent
problems. APIs state that choice in their result type.

## Explicit state threading

Appending one record uses the persistent `array.push` operation:

```forma
def require_non_empty:
    Fn(String, Diagnostics) -> Diagnostics =
    fn(value, diagnostics) {
        if value == "" {
            array.push(diagnostics, blame!(value, "must not be empty"))
        } else {
            diagnostics
        }
    };
```

Recursive and higher-order validation carries the same Array through
`array.fold`. The callback returns the next Array explicitly. Independent
checks continue after an appended record; a check that cannot continue may use
`fold_control` or Result explicitly. Ordering is deterministic evaluation and
observer order.

The input Array remains a valid immutable snapshot. Each push creates a new
logical Array whose root provenance is Generated at the push operation, while
all existing records and the appended record keep their own nested provenance.

## Reference example

`examples/explicit-diagnostics.forma` defines nested configuration records and
validators for one value and an Array of values. It returns both the unchanged
input and all collected BlameError records. The example deliberately reports
more than one problem in one execution and uses only public standard modules.

The example is a data-flow demonstration, not a universal validation
framework. It does not prescribe severity, diagnostic codes, localization,
path rendering, deduplication, or policy for turning records into Host
diagnostics. Those can be represented by application-specific record types or
explicit adapters later.

## Host boundary

Producing or returning a DiagnosticRecord has no side effect. In particular:

- `blame!` constructs one ordinary sourced value;
- `array.push` constructs one ordinary persistent Array value;
- evaluation does not add entries to the best-effort Host diagnostic sink;
- module loading does not reject a non-empty diagnostic Array; and
- a CLI, LSP, or embedding Host must explicitly select and project records
  before they become externally visible diagnostics.

This boundary prevents library validation policy from silently changing Host
execution semantics and leaves batch validation useful in ordinary pure code.

## Resource behavior

Collection uses the existing deterministic Array push and fold accounting.
Every logical append is charged independently of physical sharing. Cancellation
remains observable at the existing VM/native callback boundaries; this RFC
adds no uninterruptible traversal or quota exemption.

## Acceptance criteria

1. the reference example uses `Array(BlameError)` as ordinary explicit state;
2. one validation run can return multiple records in deterministic order;
3. nested Array validation threads state through public `array.fold`;
4. the original input value and input diagnostics Array remain unchanged;
5. every record retains its blamed data and authored rule provenance;
6. appending a later record does not rewrite earlier record provenance;
7. merely constructing and returning records publishes no Host diagnostic;
8. no accumulation syntax, effect, hidden parameter, or VM diagnostic channel
   is added; and
9. full workspace tests and strict Clippy pass.

## Implementation plan

1. add the ordinary user-space reference example;
2. test multi-error ordering, nested traversal, and immutable aliases;
3. test imported-data and authored-rule provenance for every record;
4. verify that ordinary execution exposes no implicit Host diagnostics;
5. record the implementation result and complete umbrella RFC 0107.

## Non-goals

- `yield`, `accumulate!`, Port, handler, or effect syntax;
- automatic diagnostic publication or best-effort Never production;
- a new DiagnosticRecord runtime type distinct from BlameError;
- severity, code, fix-it, localization, or rendering standards;
- parallel validation or nondeterministic merge ordering; or
- changing Result, `?`, Host failure lineage, or LSP scheduling.

## Implementation result

Implemented as `examples/explicit-diagnostics.forma`. The example aliases
DiagnosticRecord to BlameError, validates one Project and its nested Package
Array, and explicitly threads the persistent diagnostics Array through
`array.push` and `array.fold`.

The integration test imports the checked value from JSON and observes three
records in deterministic order while the input value and initial empty Array
remain unchanged. Recoverable workspace evaluation reports no diagnostics for
the ordinary returned collection. The test then explicitly selects and
publishes each record through `result.unwrap`; every resulting Host diagnostic
anchors data in the JSON source and its rule in the authored example source.
No VM operation, hidden channel, or Host publication rule was added.
