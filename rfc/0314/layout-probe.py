#!/usr/bin/env python3
"""Compare full-value singleton layouts from an unpublished tooling Wasm.

This measures sealed type widths, not dynamic array occurrence frequencies.
Allocator metadata and alias multiplicity are deliberately reported separately.
"""
import collections
import json
import sys
from pathlib import Path


def manifest(path):
    data = Path(path).read_bytes()
    assert data[:8] == b"\0asm\x01\0\0\0"

    def leb(at):
        value, shift = 0, 0
        while True:
            byte = data[at]
            at += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value, at
            shift += 7

    at = 8
    while at < len(data):
        kind = data[at]
        size, start = leb(at + 1)
        end = start + size
        if kind == 0:
            length, name = leb(start)
            if data[name:name + length] == b"telora.tooling":
                return json.loads(data[name + length:end])
        at = end
    raise ValueError("requires unpublished Wasm containing telora.tooling")


types = manifest(sys.argv[1])["types"]
counts = collections.Counter(
    types[t["arguments"][0]]["bytes"] for t in types if t["kind"] == "Array"
)
rows = []
for width, count in sorted(counts.items()):
    # Complete element preserves its source header; tag consumes another word.
    inline = 8 + ((4 + width + 7) // 8) * 8
    rows.append(dict(element_bytes=width, array_type_count=count,
                     existing_single_bytes=24 + 16 + width,
                     indirect_single_bytes=24 + width,
                     direct_inline_array_bytes=max(24, inline),
                     array_copy_increase=max(24, inline) - 24))
print(json.dumps(dict(note="static layouts; excludes allocator metadata and multiplicity",
                     rows=rows), indent=2))
