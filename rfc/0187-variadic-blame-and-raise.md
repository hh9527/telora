# RFC 0187: Variadic blame and `raise!`

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Raise consumes message and subject registers directly, without a BlameError carrier.
- Depends on: RFC 0186

## Summary

Change blame construction to message-first syntax with zero or more
heterogeneous subjects:

```forma
blame!("message")
blame!("message", value)
blame!("message", left, right)
```

The first subject is primary and later subjects are related. Subjects are
syntax operands, not a homogeneous Forma Array. Replace `reraise!` with
`raise!`; raising is the first promotion of ordinary error data and does not
imply that the value was previously raised.

## Acceptance criteria

1. the message is statically required to be String;
2. subjects may have unrelated types;
3. zero subjects retain the authored rule location;
4. `raise!` accepts exactly one BlameError and has type Never;
5. old blame argument order and `reraise!` are rejected without compatibility;
6. existing codec and entry error paths retain their provenance.

## Implementation result

The parser lowers variadic subjects into an internal Tuple stored in the
canonical BlameError `data` field; a single subject remains direct and the
zero-subject case uses an empty Tuple. Each tuple member retains its own rich
value provenance for later diagnostic-event expansion. The existing terminal
AST, LIR, bytecode, and runtime path is renamed from Reraise to Raise.
