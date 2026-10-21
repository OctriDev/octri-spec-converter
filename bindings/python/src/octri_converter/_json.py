"""Exact decimal transport, without a third-party JSON encoder dependency."""

import json
from decimal import Decimal


def loads(text):
    return json.loads(text, parse_float=Decimal, parse_int=lambda token: int(token) if len(token) < 4096 else Decimal(token))


def dumps(value):
    chunks = []
    active = set()
    size = 0
    nodes = 0

    def append(token):
        nonlocal size
        size += len(token.encode("utf-8"))
        if size > 128 * 1024 * 1024:
            raise ValueError("JSON transport exceeds 128 MiB")
        chunks.append(token)

    def write(item, depth):
        nonlocal nodes
        nodes += 1
        if depth > 256 or nodes > 4_000_000:
            raise ValueError("JSON transport exceeds nesting or occurrence limit")
        if isinstance(item, Decimal):
            if not item.is_finite():
                raise ValueError("JSON decimals must be finite")
            append(str(item))
        elif item is None or type(item) in (str, int, float, bool):
            append(json.dumps(item, ensure_ascii=False, allow_nan=False))
        elif type(item) in (dict, list, tuple):
            if id(item) in active:
                raise ValueError("Circular JSON transport")
            active.add(id(item))
            mapping = type(item) is dict
            append("{" if mapping else "[")
            entries = item.items() if mapping else enumerate(item)
            for index, (key, child) in enumerate(entries):
                if index:
                    append(",")
                if mapping:
                    if type(key) is not str:
                        raise TypeError("JSON object keys must be strings")
                    append(json.dumps(key, ensure_ascii=False) + ":")
                write(child, depth + 1)
            append("}" if mapping else "]")
            active.remove(id(item))
        else:
            raise TypeError("JSON transport accepts plain containers and numeric values")

    write(value, 0)
    return "".join(chunks)
