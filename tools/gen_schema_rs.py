#!/usr/bin/env python3
"""Generate src/generated/schema.rs from tools/modules.json.

Normalizes ActionScript read()/write() op lists into canonical wire ops:
- rotation directions are normalized to the DECODE direction
  (value = wire.rotate(decode_dir, bits)); write-side ops have their
  direction inverted at generation time so both sides share one enum.
- `lit` ops equal to the class module id are dropped (the module id is
  written by the encoder itself); other literals become `Const`.
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SRC = os.path.join(HERE, "modules.json")
OUT = os.path.join(ROOT, "src", "generated", "schema.rs")

INV = {"rol": "Ror", "ror": "Rol"}
DIRS = {"rol": "Rol", "ror": "Ror"}
WIDTHS = {8: "W8", 16: "W16", 32: "W32"}


def norm_int_op(o, side):
    """Return Int op dict with rotation normalized to decode direction."""
    w = int(o.get("width", 32))
    d = o["dir"]
    if side == "write":
        d = INV[d.lower()] if d.lower() in INV else d
        d = d.lower()
    return {
        "op": "int",
        "name": o.get("name"),
        "width": w,
        "dir": DIRS[d.lower()],
        "bits": int(o["bits"]),
    }


def norm_op(o, side):
    op = o["op"]
    if op in ("i8_rot", "i16_rot", "i32_rot"):
        return norm_int_op(o, side)
    if op == "i16":
        return {"op": "int", "name": o.get("name"), "width": 16, "dir": None}
    if op == "i8":
        return {"op": "int", "name": o.get("name"), "width": 8, "dir": None}
    if op == "i32":
        return {"op": "int", "name": o.get("name"), "width": 32, "dir": None}
    if op in ("bool", "utf", "f32", "f64"):
        return {"op": op, "name": o.get("name")}
    if op == "skip":
        w = {"i8": 8, "u8": 8, "i16": 16, "u16": 16, "i32": 32, "u32": 32}[
            str(o.get("type", "i16")).lower()
        ]
        return {"op": "skip", "width": w}
    if op == "nested":
        return {"op": "nested", "name": o.get("name")}
    if op.startswith("vec_"):
        elem = {"op": op[4:], "name": None}
        if "dir" in o:
            elem["dir"] = o["dir"]
            elem["bits"] = o["bits"]
            elem["width"] = o.get("width", 32)
        n = norm_elem(elem, side)
        cw = "W8" if str(o.get("count", "i8")) in ("i8", "u8") else "W16"
        return {"op": "vec", "name": o.get("name"), "count": cw, "elem": n}
    if op == "lit":
        w = {"i8": 8, "u8": 8, "i16": 16, "u16": 16, "i32": 32, "u32": 32}[
            str(o.get("count", o.get("type", "i16"))).lower()
        ]
        return {"op": "const", "value": int(o["value"]), "width": w}
    raise ValueError(f"unknown op {o}")


def norm_elem(e, side):
    op = e["op"]
    if op in ("i8_rot", "i16_rot", "i32_rot"):
        n = norm_int_op(e, side)
        return {"op": "int", "width": n["width"], "dir": n["dir"], "bits": n["bits"]}
    if op in ("i8", "i16", "i32", "u8", "u16", "u32"):
        w = {"i8": 8, "u8": 8, "i16": 16, "u16": 16, "i32": 32, "u32": 32}[op]
        return {"op": "int", "width": w, "dir": None}
    if op == "nested":
        return {"op": "nested"}
    if op in ("bool", "utf", "f32", "f64"):
        return {"op": op}
    raise ValueError(f"unknown vec elem {e}")


def rs_op(o):
    name = o.get("name")
    ns = f'Some("{name}")' if name else "None"
    op = o["op"]
    if op == "int":
        if o["dir"]:
            return (
                f"Op::Int {{ name: {ns}, width: IntWidth::{WIDTHS[o['width']]}, "
                f"rot: Some((RotDir::{o['dir']}, {o['bits']})) }}"
            )
        return f"Op::Int {{ name: {ns}, width: IntWidth::{WIDTHS[o['width']]}, rot: None }}"
    if op in ("bool", "utf", "f32", "f64"):
        return f"Op::{op.capitalize()} {{ name: {ns} }}"
    if op == "skip":
        return f"Op::Skip {{ width: IntWidth::{WIDTHS[o['width']]} }}"
    if op == "nested":
        return f"Op::Nested {{ name: {ns} }}"
    if op == "vec":
        return (
            f"Op::Vec {{ name: {ns}, count: IntWidth::{o['count']}, "
            f"elem: {rs_elem(o['elem'])} }}"
        )
    if op == "const":
        return f"Op::Const {{ value: {o['value']}, width: IntWidth::{WIDTHS[o['width']]} }}"
    raise ValueError(op)


def rs_elem(e):
    op = e["op"]
    if op == "int":
        if e["dir"]:
            return (
                f"Elem::Int {{ width: IntWidth::{WIDTHS[e['width']]}, "
                f"rot: Some((RotDir::{e['dir']}, {e['bits']})) }}"
            )
        return f"Elem::Int {{ width: IntWidth::{WIDTHS[e['width']]}, rot: None }}"
    if op == "nested":
        return "Elem::Nested"
    return f"Elem::{op.capitalize()}"


def emit_list(ops, var, f):
    f.write(f"static {var}: &[Op] = &[\n")
    for o in ops:
        f.write(f"    {rs_op(o)},\n")
    f.write("];\n")


def main():
    cat = [c for c in json.load(open(SRC, encoding="utf-8")) if c.get("module_id") is not None]
    seen = {}
    for c in cat:
        mid = c["module_id"]
        if mid in seen and seen[mid] != c["name"]:
            print(f"warning: id {mid} used by {seen[mid]} and {c['name']}", file=sys.stderr)
        seen[mid] = c["name"]

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    f = open(OUT, "w", encoding="utf-8", newline="\n")
    f.write(
        "// @generated by tools/gen_schema_rs.py from tools/modules.json.\n"
        "// Do not edit by hand.\n"
        "use crate::op::{Elem, IntWidth, Op, RotDir};\n\n"
        "/// Static decode/encode schema for one module id.\n"
        "pub struct ModuleSchema {\n"
        "    pub id: i16,\n"
        "    pub name: &'static str,\n"
        "    /// Ops for decoding an incoming body (wire order).\n"
        "    pub read: &'static [Op],\n"
        "    /// Ops for encoding an outgoing body (wire order).\n"
        "    pub write: &'static [Op],\n"
        "}\n\n"
    )

    schemas = sorted(cat, key=lambda c: c["module_id"])
    for i, c in enumerate(schemas):
        read_ops, write_ops = [], []
        try:
            for o in c.get("read_ops") or []:
                read_ops.append(norm_op(o, "read"))
            for o in c.get("write_ops") or []:
                n = norm_op(o, "write")
                # drop the class's own module-id write; encoder prepends it
                if n["op"] == "const" and n["value"] == c["module_id"] and n["width"] == 16:
                    continue
                write_ops.append(n)
        except Exception as e:
            print(f"error in {c['name']}: {e}", file=sys.stderr)
            raise
        emit_list(read_ops, f"R{i}", f)
        emit_list(write_ops, f"W{i}", f)
        f.write(
            f'// id={c["module_id"]}\n'
            f'const S{i}: ModuleSchema = ModuleSchema {{ id: {c["module_id"]}, '
            f'name: "{c["name"]}", read: R{i}, write: W{i} }};\n\n'
        )

    f.write("/// All known module schemas sorted by id (binary-searchable).\n")
    f.write("pub static SCHEMAS: &[ModuleSchema] = &[\n")
    for i in range(len(schemas)):
        f.write(f"    S{i},\n")
    f.write("];\n")
    f.close()
    print(f"wrote {OUT} with {len(schemas)} schemas")


if __name__ == "__main__":
    main()
