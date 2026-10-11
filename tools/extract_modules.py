#!/usr/bin/env python3
"""Extract wire schemas from the decompiled DarkOrbit protocol classes.

Parses every `*.as` under the module directories and emits a JSON catalog:
    { name, module_id, file, read_ops: [...], write_ops: [...], errors: [...] }

Final op vocabulary (after normalization):
    i8 u8 i16 u16 i32 u32 f32 f64 bool utf
    i8_rot u8_rot i16_rot u16_rot i32_rot u32_rot   {dir,bits,width}
    nested        - i16 module id then nested module body (0 = null)
    vec_<prim>    - count (u8/i8/i16/i32) then N primitives
    vec_nested    - count then N nested modules
    vec_i32_rot   - count then N rotated ints
    skip_i16      - discarded short (e.g. parent tag consumed on read)
    lit_i16       - literal short (module id / parent id on write)
    super         - parent class read/write (resolved in post)
    unknown       - fallback raw tail
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

DEFAULT_ROOT = pathlib.Path(
    r"C:\Users\Yagiz\Desktop\DOBOT\game_resources\scripts\net\bigpoint\darkorbit\com\module"
)
# Workspace fallback when DOBOT tree is absent.
_REPOS_ROOT = pathlib.Path(__file__).resolve().parents[2] / (
    "game_resources/scripts/net/bigpoint/darkorbit/com/module"
)
ROOT = DEFAULT_ROOT if DEFAULT_ROOT.is_dir() else _REPOS_ROOT

READ_FN = re.compile(
    r"(?:override\s+)?public function read\(param1:IDataInput\)\s*:\s*void\s*\{", re.M
)
WRITE_FN = re.compile(
    r"(?:override\s+)?public function write\(param1:IDataOutput\)\s*:\s*void\s*\{", re.M
)
CLASS_RE = re.compile(r"public class (\w+)(?:\s+extends\s+(\w+))?")
MODID_RE = re.compile(r"function getLibcomModuleId\(\)\s*:\s*int\s*\{\s*return (-?\d+)")

PRIM_READS = {
    "readByte": "i8", "readUnsignedByte": "u8", "readShort": "i16",
    "readUnsignedShort": "u16", "readInt": "i32", "readUnsignedInt": "u32",
    "readBoolean": "bool", "readFloat": "f32", "readDouble": "f64",
    "readUTF": "utf",
}
PRIM_WRITES = {
    "writeByte": "i8", "writeShort": "i16", "writeInt": "i32",
    "writeUnsignedInt": "u32", "writeBoolean": "bool", "writeFloat": "f32",
    "writeDouble": "f64", "writeUTF": "utf",
}
COUNT_FOR_CALL = {"writeByte": "i8", "writeShort": "i16", "writeInt": "i32"}

CREATE_RE = re.compile(
    r"(?:this\.)?(\w+)\s*=\s*\w+\(ModuleFactory\.get\(\)\.createInstance\(param1\.readShort\(\)\)\)"
)
BARE_CREATE_RE = re.compile(
    r"=\s*\w+\(ModuleFactory\.get\(\)\.createInstance\(param1\.readShort\(\)\)\)"
)


def block_of(src: str, start: int) -> str:
    assert src[start] == "{"
    depth = 0
    for i in range(start, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[start + 1 : i]
    raise ValueError("unbalanced braces")


def split_statements(body: str):
    body = re.sub(r"//[^\n]*", "", body)
    nodes, cur, i, n = [], [], 0, len(body)
    while i < n:
        c = body[i]
        if c == "{":
            depth, j = 1, i + 1
            while j < n and depth:
                if body[j] == "{":
                    depth += 1
                elif body[j] == "}":
                    depth -= 1
                j += 1
            prefix = "".join(cur).strip()
            cur = []
            nodes.append(("block", prefix, body[i + 1 : j - 1]))
            i = j
        elif c == ";":
            text = "".join(cur).strip()
            cur = []
            if text:
                nodes.append(("stmt", text))
            i += 1
        else:
            cur.append(c)
            i += 1
    tail = "".join(cur).strip()
    if tail:
        nodes.append(("stmt", tail))
    return nodes


def parse_rotation(expr: str):
    """Parse AS rotation expressions -> (dir, bits, width)."""
    e = expr.replace(" ", "")
    e = e.replace("0xFFFF&", "").replace("0xFF&", "")
    e = e.replace("(", "").replace(")", "")
    for width in (32, 16, 8):
        m = re.match(
            rf"^(?:this\.)?(\w+)>>>(\d+)%{width}\|(?:this\.)?\1<<{width}-(\d+)%{width}$",
            e,
        )
        if m and m.group(2) == m.group(3):
            return ("ror", int(m.group(2)), width)
        m = re.match(
            rf"^(?:this\.)?(\w+)<<(\d+)%{width}\|(?:this\.)?\1>>>{width}-(\d+)%{width}$",
            e,
        )
        if m and m.group(2) == m.group(3):
            return ("rol", int(m.group(2)), width)
    return None


def rot_var(expr: str) -> str | None:
    # Prefer an explicit this.field reference anywhere in the expression;
    # mask constants like 0xFFFF must never become the field name.
    m = re.search(r"\bthis\.(\w+)", expr)
    if m:
        return m.group(1)
    m = re.match(r"^(?:\(0xFFFF & \()?(?:this\.)?([A-Za-z_]\w*)", expr.strip())
    return m.group(1) if m else None


def parse_nodes(nodes, ops, errors, direction):
    for node in nodes:
        if node[0] == "block":
            _k, prefix, inner = node
            inner_nodes = split_statements(inner)
            p = prefix.replace(" ", "")
            if p.startswith("while(this.") and ".length>0" in p:
                continue  # vector clear
            if p.startswith("while(") or p.startswith("foreach(") or p.startswith("for("):
                body_ops: list = []
                parse_nodes(inner_nodes, body_ops, errors, direction)
                ops.append({"op": "loop", "body": body_ops})
                continue
            if p.startswith("if(null!=this."):
                fname = re.match(r"^if\(null!=this\.(\w+)\)", p).group(1)
                body_ops = []
                parse_nodes(inner_nodes, body_ops, errors, direction)
                ops.append({"op": "if_nn", "name": fname, "body": body_ops})
                continue
            if p.startswith("if(null==this."):
                fname = re.match(r"^if\(null==this\.(\w+)\)", p).group(1)
                body_ops = []
                parse_nodes(inner_nodes, body_ops, errors, direction)
                ops.append({"op": "if_null", "name": fname, "body": body_ops})
                continue
            if p.startswith("if(") or p.startswith("else"):
                body_ops = []
                parse_nodes(inner_nodes, body_ops, errors, direction)
                ops.append({"op": "if", "cond": prefix[:80], "body": body_ops})
                continue
            if p.startswith("switch") or p.startswith("case") or p.startswith("default") or p.startswith("try"):
                body_ops = []
                parse_nodes(inner_nodes, body_ops, errors, direction)
                ops.append({"op": "block", "cond": prefix[:80], "body": body_ops})
                continue
            errors.append(f"unhandled block: {prefix[:80]}")
            continue

        stmt = node[1]
        s = " ".join(stmt.split())

        if re.match(r"^var\b", s):
            continue
        if s in ("super(param1);", "return", "return;", "break", "break;", "continue", "continue;"):
            continue
        if s in ("super.read(param1)", "super.write(param1)"):
            ops.append({"op": "super"})
            continue
        if s.startswith("super("):
            continue

        # _locN_ = param1.readX();
        m = re.match(r"^(_loc\d+_)\s*=\s*param1\.(\w+)\(\);?$", s)
        if m and m.group(2) in PRIM_READS:
            ops.append({"op": "local", "name": m.group(1), "type": PRIM_READS[m.group(2)]})
            continue

        # this.f = param1.readX();
        m = re.match(r"^(?:this\.)?(\w+)\s*=\s*param1\.(\w+)\(\);?$", s)
        if m and m.group(2) in PRIM_READS:
            ops.append({"op": PRIM_READS[m.group(2)], "name": m.group(1)})
            continue

        # discard: param1.readX();
        m = re.match(r"^param1\.(\w+)\(\);?$", s)
        if m and m.group(1) in PRIM_READS:
            ops.append({"op": "skip", "type": PRIM_READS[m.group(1)]})
            continue

        # param1.writeX(...)
        m = re.match(r"^param1\.(\w+)\((.*)\);?$", s)
        if m and m.group(1) in PRIM_WRITES:
            call, arg = m.group(1), m.group(2).strip()
            op_name = PRIM_WRITES[call]
            if re.match(r"^-?\d+$", arg):
                ops.append({"op": "lit", "type": op_name, "value": int(arg)})
                continue
            fm = re.match(r"^(?:this\.)?(\w+)$", arg)
            if fm:
                ops.append({"op": op_name, "name": fm.group(1)})
                continue
            lm = re.match(r"^(?:this\.)?(\w+)\.length$", arg)
            if lm and call in COUNT_FOR_CALL:
                ops.append({"op": "count", "name": lm.group(1), "type": COUNT_FOR_CALL[call]})
                continue
            rot = parse_rotation(arg)
            if rot:
                ops.append({"op": op_name + "_rot", "name": rot_var(arg),
                            "dir": rot[0], "bits": rot[1], "width": rot[2]})
                continue
            if re.match(r"^int\((?:this\.)?\w+\)$", arg):
                inner = re.match(r"^int\((?:this\.)?(\w+)\)$", arg).group(1)
                ops.append({"op": op_name, "name": inner})
                continue
            ops.append({"op": "write_expr", "call": call, "arg": arg[:140]})
            errors.append(f"write expr: {s[:140]}")
            continue

        # nested decl: this.f = T(ModuleFactory...createInstance(param1.readShort()));
        if BARE_CREATE_RE.search(s):
            nm = re.match(r"^(?:this\.)?(\w+)\s*=", s)
            ops.append({"op": "nested_decl", "name": nm.group(1) if nm else None})
            continue

        m = re.match(r"^(?:this\.)?(\w+)\.push\((.*)\);?$", s)
        if m:
            ops.append({"op": "push", "name": m.group(1), "arg": m.group(2)[:80]})
            continue

        m = re.match(r"^(_loc\d+_|this\.(\w+))\.read\(param1\);?$", s)
        if m:
            ops.append({"op": "nested_read", "name": m.group(2) or m.group(1)})
            continue

        m = re.match(r"^(_loc\d+_|this\.(\w+))\.write\(param1\);?$", s)
        if m:
            ops.append({"op": "nested_write", "name": m.group(2) or m.group(1)})
            continue

        # self-assignment / transforms on a field
        m = re.match(r"^(?:this\.)?(\w+)\s*=\s*(.+)$", s)
        if m:
            fname, expr = m.group(1), m.group(2)
            rot = parse_rotation(expr)
            if rot and re.search(rf"\b{fname}\b", expr):
                for prev in reversed(ops):
                    if prev.get("name") == fname and prev["op"] in (
                        "i8", "u8", "i16", "u16", "i32", "u32"):
                        prev["op"] = prev["op"] + "_rot"
                        prev["dir"], prev["bits"], prev["width"] = rot
                        break
                else:
                    ops.append({"op": "rot", "name": fname,
                                "dir": rot[0], "bits": rot[1], "width": rot[2]})
                continue
            if re.match(r"^param1\.", expr):
                errors.append(f"unhandled call assign: {s[:120]}")
                continue
            # numeric post-processing (masks, sign fixes, int casts)
            continue

        if re.match(r"^(_loc\d+|before|streamLength|command|input|msgByteArray)", s):
            continue
        if re.match(r"^(?:this\.)?\w+\.(pop|splice|clear)\(", s):
            continue
        errors.append(f"unhandled: {s[:140]}")


def finalize(ops, errors, direction):
    """Normalize nested-module and loop constructs into flat ops."""
    out = []
    i = 0
    while i < len(ops):
        op = ops[i]
        if op["op"] == "local":
            # may be a collection count for a following loop, or vec elem temp
            nxt = ops[i + 1] if i + 1 < len(ops) else None
            if nxt and nxt["op"] == "loop":
                # classify the loop body
                body = nxt["body"]
                vec_op = classify_loop(body, op["type"], direction, errors)
                out.append(vec_op)
                i += 2
                continue
            out.append(op)
            i += 1
            continue
        if op["op"] == "loop":
            vec_op = classify_loop(op["body"], None, direction, errors)
            out.append(vec_op)
            i += 1
            continue
        if op["op"] == "nested_decl":
            # possibly followed by if_nn/if_null containing nested_read
            name = op.get("name")
            nxt = ops[i + 1] if i + 1 < len(ops) else None
            nxt2 = ops[i + 2] if i + 2 < len(ops) else None
            body_reads = None
            skip = 1
            if nxt and nxt["op"] == "if_nn" and (nxt.get("name") == name):
                body_reads = nxt["body"]
                if nxt2 and nxt2["op"] == "if" and "else" in nxt2.get("cond", ""):
                    skip = 3
                else:
                    skip = 2
            if body_reads is not None and any(
                    b["op"] == "nested_read" for b in body_reads):
                out.append({"op": "nested", "name": name})
                i += skip
                continue
            # declaration without read (should not normally happen)
            out.append({"op": "nested", "name": name})
            i += 1
            continue
        if op["op"] == "if_nn":
            body = op["body"]
            if any(b["op"] == "nested_read" for b in body):
                out.append({"op": "nested", "name": op.get("name")})
            elif any(b["op"] == "nested_write" for b in body):
                out.append({"op": "nested", "name": op.get("name")})
            else:
                out.append({"op": "if", "name": op.get("name"), "body": body})
            i += 1
            continue
        if op["op"] in ("nested_read", "nested_write"):
            out.append({"op": "nested", "name": op.get("name")})
            i += 1
            continue
        if op["op"] == "if" and any(b["op"] in ("nested_read", "nested_write") for b in op["body"]):
            out.append({"op": "nested", "name": None})
            i += 1
            continue
        out.append(op)
        i += 1

    # Merge write-side artifacts:
    #   "count"(name=X) followed by "vec_*"(name=None) -> name the vec op after X
    #   "nested" followed by if(else){lit 0} -> drop the null-branch artifact
    merged = []
    i = 0
    while i < len(out):
        op = out[i]
        nxt = out[i + 1] if i + 1 < len(out) else None
        if op["op"] == "count" and nxt and nxt["op"].startswith("vec_"):
            nxt = dict(nxt)
            nxt["name"] = op.get("name")
            nxt["count"] = op.get("type")
            merged.append(nxt)
            i += 2
            continue
        if op["op"] == "nested" and nxt and nxt["op"] == "if" \
                and all(b["op"] == "lit" and b.get("value") == 0 for b in nxt["body"]):
            merged.append(op)
            i += 2
            continue
        merged.append(op)
        i += 1
    return merged


def classify_loop(body, count_type, direction, errors):
    """Classify a collection loop body into a vec_* op."""
    nested = any(b["op"] in ("nested", "nested_decl", "nested_read", "nested_write") for b in body)
    pushes = [b for b in body if b["op"] == "push"]
    rots = [b for b in body if b["op"] == "rot" or b["op"].endswith("_rot")]
    locals_ = [b for b in body if b["op"] == "local"]
    target = pushes[0]["name"] if pushes else None
    if nested:
        return {"op": "vec_nested", "name": target, "count": count_type or "i8"}
    for p in pushes:
        arg = p.get("arg", "")
        pm = re.match(r"^param1\.(\w+)\(", arg)
        if pm and pm.group(1) in PRIM_READS:
            return {"op": "vec_" + PRIM_READS[pm.group(1)], "name": p["name"],
                    "count": count_type or "i8"}
    if locals_:
        t = locals_[0]["type"]
        if rots and t == "i32":
            r = rots[0]
            return {"op": "vec_i32_rot", "name": target, "count": count_type or "i8",
                    "dir": r.get("dir"), "bits": r.get("bits"), "width": r.get("width")}
        return {"op": "vec_" + t, "name": target, "count": count_type or "i8"}
    # writes: param1.writeX(_loc_ [rot]) or nested_write
    for b in body:
        if b["op"] == "nested_write":
            return {"op": "vec_nested", "name": target, "count": count_type or "i8"}
        if b["op"].endswith("_rot"):
            base = b["op"][: -len("_rot")]
            return {"op": f"vec_{base}_rot", "name": target, "count": count_type or "i8",
                    "dir": b.get("dir"), "bits": b.get("bits"), "width": b.get("width")}
        if b["op"] in PRIM_READS.values() or b["op"] in PRIM_WRITES.values():
            return {"op": "vec_" + b["op"].replace("_rot", ""), "name": target,
                    "count": count_type or "i8"}
    errors.append(f"unclassified loop body: {body[:3]}")
    return {"op": "vec_unknown", "name": target, "count": count_type or "i8"}


def resolve_super(catalog):
    by_name = {c["name"]: c for c in catalog if c.get("name")}
    def body_ops(c, key):
        ops, seen = [], set()
        def walk(cls):
            if not cls or cls["name"] in seen:
                return
            seen.add(cls["name"])
            for op in cls.get(key, []):
                if op["op"] == "super":
                    walk(by_name.get(cls.get("parent")))
                else:
                    ops.append(op)
        walk(c)
        return ops
    for c in catalog:
        for key in ("read_ops", "write_ops"):
            c[key] = body_ops(c, key)


def parse_file(path: pathlib.Path):
    src = path.read_text(encoding="utf-8", errors="replace")
    cm = CLASS_RE.search(src)
    name = cm.group(1) if cm else path.stem
    parent = cm.group(2) if cm else None
    mid = MODID_RE.search(src)
    module_id = int(mid.group(1)) if mid else None
    out = {"name": name, "module_id": module_id, "parent": parent,
           "file": path.name, "read_ops": [], "write_ops": [], "errors": []}
    for fn_re, key in ((READ_FN, "read_ops"), (WRITE_FN, "write_ops")):
        m = fn_re.search(src)
        if not m:
            continue
        body = block_of(src, m.end() - 1)
        nodes = split_statements(body)
        raw: list = []
        parse_nodes(nodes, raw, out["errors"], key)
        out[key] = finalize(raw, out["errors"], key)
    return out


def _parse_args(argv: list[str]):
    """Support `--root` / `--out` and a positional out path for back-compat."""
    root = ROOT
    out = pathlib.Path("modules.json")
    args = list(argv)
    i = 0
    positionals: list[str] = []
    while i < len(args):
        a = args[i]
        if a in ("--root", "-r") and i + 1 < len(args):
            root = pathlib.Path(args[i + 1])
            i += 2
        elif a in ("--out", "-o") and i + 1 < len(args):
            out = pathlib.Path(args[i + 1])
            i += 2
        elif a.startswith("-"):
            print(f"unknown flag: {a}", file=sys.stderr)
            return None
        else:
            positionals.append(a)
            i += 1
    if positionals:
        out = pathlib.Path(positionals[0])
    return root, out


def main(argv: list[str] | None = None) -> int:
    parsed = _parse_args(sys.argv[1:] if argv is None else argv)
    if parsed is None:
        return 2
    root, out = parsed
    if not root.is_dir():
        print(f"module root not found: {root}", file=sys.stderr)
        return 1
    files = sorted(root.rglob("*.as"))
    catalog = []
    for f in files:
        try:
            catalog.append(parse_file(f))
        except Exception as e:
            catalog.append({"name": f.stem, "file": f.name, "error": str(e)})
    resolve_super(catalog)

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(catalog, indent=1), encoding="utf-8")
    n_err = sum(1 for c in catalog if c.get("errors"))
    n_id = sum(1 for c in catalog if c.get("module_id") is not None)
    print(f"{len(catalog)} classes, {n_id} with module id, {n_err} with parse errors")
    for c in catalog:
        if c.get("errors"):
            print("ERR", c["name"], c["errors"][:4])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
