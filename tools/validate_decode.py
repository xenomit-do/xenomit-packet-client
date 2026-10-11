#!/usr/bin/env python3
"""Validate extracted module schemas by decoding a real reassembled stream."""
import json
import struct
import sys
import collections
import pathlib

CAT = {c["module_id"]: c for c in json.load(open("tools/modules.json")) if c.get("module_id") is not None}
BY_NAME = {c["name"]: c for c in json.load(open("tools/modules.json"))}


def rol(v, n, w=32):
    m = (1 << w) - 1
    v &= m
    return ((v << n) | (v >> (w - n))) & m


def ror(v, n, w=32):
    m = (1 << w) - 1
    v &= m
    return ((v >> n) | (v << (w - n))) & m


def sign(v, w):
    return v - (1 << w) if v & (1 << (w - 1)) else v


class R:
    def __init__(self, b):
        self.b = b
        self.p = 0

    def take(self, n):
        if self.p + n > len(self.b):
            raise EOFError(f"need {n} @{self.p} len {len(self.b)}")
        v = self.b[self.p:self.p + n]
        self.p += n
        return v

    def u8(self):
        return self.take(1)[0]

    def i8(self):
        return sign(self.u8(), 8)

    def u16(self):
        return struct.unpack(">H", self.take(2))[0]

    def i16(self):
        return sign(self.u16(), 16)

    def u32(self):
        return struct.unpack(">I", self.take(4))[0]

    def i32(self):
        return sign(self.u32(), 32)

    def f32(self):
        return struct.unpack(">f", self.take(4))[0]

    def f64(self):
        return struct.unpack(">d", self.take(8))[0]

    def bool(self):
        return self.u8() != 0

    def utf(self):
        n = self.u16()
        return self.take(n).decode("utf-8", "replace")

    @property
    def left(self):
        return len(self.b) - self.p


def decode_ops(r, ops, depth=0):
    fields = {}
    seq = 0
    for op in ops:
        k = op["op"]
        name = op.get("name") or f"#{seq}"
        seq += 1
        if k == "i8":
            fields[name] = r.i8()
        elif k == "u8":
            fields[name] = r.u8()
        elif k == "i16":
            fields[name] = r.i16()
        elif k == "u16":
            fields[name] = r.u16()
        elif k == "i32":
            fields[name] = r.i32()
        elif k == "u32":
            fields[name] = r.u32()
        elif k == "bool":
            fields[name] = r.bool()
        elif k == "utf":
            fields[name] = r.utf()
        elif k == "f32":
            fields[name] = r.f32()
        elif k == "f64":
            fields[name] = r.f64()
        elif k in ("i8_rot", "i16_rot", "u16_rot", "i32_rot", "u32_rot"):
            w = op["width"]
            raw = {8: r.u8, 16: r.u16, 32: r.u32}[w]()
            v = ror(raw, op["bits"], w) if op["dir"] == "ror" else rol(raw, op["bits"], w)
            fields[name] = sign(v, w) if not k.startswith("u") else v
        elif k == "i16_rot":
            fields[name] = sign(r.u16(), 16)
        elif k == "nested":
            mid = r.i16()
            if mid == 0:
                fields[name] = None
            else:
                c = CAT.get(mid)
                if not c:
                    raise ValueError(f"unknown nested module {mid} for {name}")
                fields[name] = {"_mid": mid, "_name": c["name"],
                                **decode_ops(r, c["read_ops"], depth + 1)}
        elif k.startswith("vec_"):
            ct = op.get("count") or "i8"
            n = {"i8": r.i8, "u8": r.u8, "i16": r.i16, "u16": r.u16,
                 "i32": r.i32, "u32": r.u32}[ct]()
            if n < 0 or n > 1 << 20:
                raise ValueError(f"bad vec count {n} for {name}")
            elem = k[4:]
            items = []
            for _ in range(n):
                if elem == "nested":
                    mid = r.i16()
                    c = CAT.get(mid)
                    if not c:
                        raise ValueError(f"unknown nested vec module {mid} for {name}")
                    items.append({"_mid": mid, "_name": c["name"],
                                  **decode_ops(r, c["read_ops"], depth + 1)})
                elif elem == "utf":
                    items.append(r.utf())
                elif elem == "i32_rot":
                    raw = r.u32()
                    v = ror(raw, op["bits"], 32) if op["dir"] == "ror" else rol(raw, op["bits"], 32)
                    items.append(sign(v, 32))
                elif elem == "i16_rot":
                    raw = r.u16()
                    v = ror(raw, op["bits"], 16) if op["dir"] == "ror" else rol(raw, op["bits"], 16)
                    items.append(sign(v, 16))
                elif elem == "i8_rot":
                    raw = r.u8()
                    v = ror(raw, op["bits"], 8) if op["dir"] == "ror" else rol(raw, op["bits"], 8)
                    items.append(sign(v, 8))
                elif elem in ("i8", "u8", "i16", "u16", "i32", "u32", "f32", "f64", "bool"):
                    items.append(getattr(r, elem)())
                else:
                    raise ValueError(f"unsupported vec elem {elem}")
            fields[name] = items
        elif k == "skip":
            getattr(r, op["type"])()
        elif k == "lit":
            getattr(r, op["type"])()
        elif k == "local":
            getattr(r, op["type"])()
        else:
            raise ValueError(f"unhandled op {k}")
    return fields


def decode_body(body):
    r = R(body)
    mid = r.i16()
    c = CAT.get(mid)
    if not c:
        return mid, None, r.left, "no schema"
    try:
        fields = decode_ops(r, c["read_ops"])
        return mid, c["name"], r.left, fields
    except Exception as e:
        return mid, c["name"], r.left, f"ERROR: {e}"


def main():
    stream = pathlib.Path(sys.argv[1]).read_bytes()
    msgs, pos = [], 0
    while pos + 3 <= len(stream):
        n = int.from_bytes(stream[pos:pos + 3], "big")
        if pos + 3 + n > len(stream):
            break
        msgs.append(stream[pos + 3:pos + 3 + n])
        pos += 3 + n
    print(f"{len(msgs)} messages")
    stats = collections.Counter()
    seen = {}
    for i, body in enumerate(msgs):
        mid, name, left, res = decode_body(body)
        stats[(mid, name)] += 1
        if isinstance(res, str) or left != 0:
            key = (mid, name)
            if key not in seen or str(res) != str(seen[key][1]):
                seen[key] = (i, res, left, body[:60].hex())
    print("\n== module frequency ==")
    for (mid, name), n in stats.most_common(40):
        print(f"{n:5d} {mid:7d} {name}")
    print("\n== issues ==")
    for (mid, name), (i, res, left, hx) in sorted(seen.items()):
        print(f"msg#{i} {name}({mid}) left={left} res={res if isinstance(res,str) else 'ok'} hex={hx}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
