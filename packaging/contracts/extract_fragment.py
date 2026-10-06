"""Extract a small, self-contained fragment of a provider's published spec
for the engine contract tests (#275, crates/sonara-engine/tests/contracts):
the named operations (parameters, request body, responses) of an OpenAPI 3
or Swagger 2 document, or the named methods of a Google discovery document,
plus every schema they reference, refs rewritten to #/$defs/<name>.
Documentation-only keys are dropped and long descriptions cut.

usage:
  python packaging/contracts/extract_fragment.py openapi <spec> <out.json> "<METHOD> <path>" ...
      [--prune A,B]  (refs to these schemas become a "pruned" note)
  python packaging/contracts/extract_fragment.py discovery <spec> <out.json> <resource.method> ...
      [--keep A,B]   (only these schemas are kept; other refs become a note)

Discovery documents state "Required." and "in the range [a, b]" in prose:
those become `required` and `minimum`/`maximum`, marked `x-derived`. The
commands used per provider are in each fragment's SOURCES.md. A YAML spec
needs PyYAML.
"""

from __future__ import annotations

import json
import re
import sys

KEEP_DESC = 160


def load(path):
    with open(path, encoding="utf8") as f:
        if path.endswith((".yml", ".yaml")):
            import yaml

            return yaml.safe_load(f)
        return json.load(f)


DROP = (
    "examples",
    "example",
    "x-fern-examples",
    "x-oaiMeta",
    "x-codeSamples",
    "x-readme",
    "x-fern-sdk-group-name",
    "x-fern-sdk-method-name",
    "x-fern-audiences",
    "title",
    "x-stainless-const",
    "x-oaiTypeLabel",
    "externalDocs",
    "tags",
    "x-mint",
    "x-speakeasy-name-override",
    "enumDescriptions",
    "x-fern-type-name",
    "deprecated",
    "operationId",
    "summary",
)
PRUNE = set()
KEEP = set()
RANGE = re.compile(r"in the range \[(-?[0-9.]+), (-?[0-9.]+)\]")


def trim(node, names=False):
    """Drop documentation-only keys; `names`: the keys are property names."""
    if isinstance(node, dict):
        out = {}
        for k, v in node.items():
            if not names and k in DROP:
                continue
            if not names and k == "description" and isinstance(v, str):
                v = v if len(v) <= KEEP_DESC else v[:KEEP_DESC].rstrip() + "..."
            out[k] = trim(
                v,
                names=(
                    not names
                    and k
                    in ("properties", "$defs", "definitions", "schemas", "parameters")
                    and isinstance(v, dict)
                ),
            )
        return out
    if isinstance(node, list):
        return [trim(x) for x in node]
    return node


def derive(schema):
    """Discovery documents say `Required.` and `in the range [a, b]` in prose:
    turn them into `required` and `minimum`/`maximum` (marked x-derived)."""
    props = schema.get("properties") or {}
    req = []
    for name, prop in props.items():
        d = prop.get("description", "")
        if d.startswith("Required."):
            req.append(name)
        m = RANGE.search(d)
        if m and prop.get("type") in ("number", "integer"):
            prop["minimum"] = float(m.group(1))
            prop["maximum"] = float(m.group(2))
            prop["x-derived"] = "minimum/maximum from the description"
    if req:
        schema["required"] = req
        schema["x-derived"] = "required from 'Required.' in the descriptions"
    return schema


def openapi(spec, ops):
    comps = spec.get("components", {})
    defs = {}
    queue = []

    def rewrite(node):
        if isinstance(node, dict):
            out = {}
            for k, v in node.items():
                if k == "$ref" and isinstance(v, str):
                    m = re.match(
                        r"#/components/(schemas|responses|parameters|requestBodies)/(.+)",
                        v,
                    )
                    d2 = re.match(r"#/definitions/(.+)", v)
                    if d2 and not m:
                        name = d2.group(1)
                        if name not in defs:
                            defs[name] = None
                            queue.append(("definitions", name, name))
                        out[k] = f"#/$defs/{name}"
                        continue
                    if m:
                        name = (
                            m.group(2)
                            if m.group(1) == "schemas"
                            else f"{m.group(1)}.{m.group(2)}"
                        )
                        if name in PRUNE:
                            out.update(
                                {"description": f"pruned: {name} (not read by Sonara)"}
                            )
                            continue
                        if name not in defs:
                            defs[name] = None
                            queue.append((m.group(1), m.group(2), name))
                        out[k] = f"#/$defs/{name}"
                        continue
                out[k] = rewrite(v)
            return out
        if isinstance(node, list):
            return [rewrite(x) for x in node]
        return node

    out_ops = {}
    for op in ops:
        method, path = op.split(" ", 1)
        item = spec["paths"][path]
        o = dict(item[method.lower()])
        if "parameters" in item:
            o["parameters"] = item["parameters"] + o.get("parameters", [])
        keep = {
            k: o[k]
            for k in ("parameters", "requestBody", "responses", "security")
            if k in o
        }
        out_ops[op] = rewrite(trim(keep))
    while queue:
        section, key, name = queue.pop()
        src = spec["definitions"] if section == "definitions" else comps[section]
        defs[name] = rewrite(trim(src[key]))
    result = {"operations": out_ops, "$defs": dict(sorted(defs.items()))}
    if "securitySchemes" in comps:
        result["securitySchemes"] = trim(comps["securitySchemes"])
    if spec.get("security"):
        result["security"] = spec["security"]
    if spec.get("servers"):
        result["servers"] = spec["servers"]
    return result


def discovery(spec, methods, extra):
    schemas = spec["schemas"]
    defs = {}
    queue = list(extra)

    def rewrite(node):
        if isinstance(node, dict):
            out = {}
            for k, v in node.items():
                if k == "$ref" and isinstance(v, str):
                    if KEEP and v not in KEEP:
                        out.update({"description": f"pruned: {v} (not used by Sonara)"})
                        continue
                    if v not in defs and v not in queue:
                        queue.append(v)
                    out[k] = f"#/$defs/{v}"
                    continue
                out[k] = rewrite(v)
            return out
        if isinstance(node, list):
            return [rewrite(x) for x in node]
        return node

    def find(dotted):
        parts = dotted.split(".")
        node = spec
        for p in parts[:-1]:
            node = node["resources"][p]
        return node["methods"][parts[-1]]

    out = {}
    for m in methods:
        meth = find(m)
        keep = {
            k: meth[k]
            for k in (
                "httpMethod",
                "path",
                "flatPath",
                "parameters",
                "request",
                "response",
            )
            if k in meth
        }
        out[m] = rewrite(trim(keep))
    while queue:
        name = queue.pop()
        if name in defs:
            continue
        defs[name] = None
        defs[name] = derive(rewrite(trim(schemas[name])))
    return {
        "rootUrl": spec.get("rootUrl"),
        "servicePath": spec.get("servicePath"),
        "revision": spec.get("revision"),
        "parameters": trim(
            {k: v for k, v in spec.get("parameters", {}).items() if k in ("key", "alt")}
        ),
        "methods": out,
        "$defs": dict(sorted(defs.items())),
    }


def main():
    kind, src, dst, *rest = sys.argv[1:]
    if "--prune" in rest:
        i = rest.index("--prune")
        PRUNE.update(rest[i + 1].split(","))
        rest = rest[:i] + rest[i + 2 :]
    if "--keep" in rest:
        i = rest.index("--keep")
        KEEP.update(rest[i + 1].split(","))
        rest = rest[:i] + rest[i + 2 :]
    spec = load(src)
    if kind == "openapi":
        res = openapi(spec, rest)
        res["info"] = {
            "title": spec["info"].get("title"),
            "version": spec["info"].get("version"),
            "openapi": spec.get("openapi") or spec.get("swagger"),
        }
    else:
        extra = []
        if "--schemas" in rest:
            i = rest.index("--schemas")
            extra = rest[i + 1].split(",")
            rest = rest[:i]
        res = discovery(spec, rest, extra)
    with open(dst, "w", encoding="utf8", newline="\n") as f:
        json.dump(res, f, indent=1, ensure_ascii=False, sort_keys=False)
        f.write("\n")
    print(dst, len(json.dumps(res)))


main()
