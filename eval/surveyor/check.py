"""Validate report structure/citations, never architectural truth. Local, no API calls."""
import argparse
import json
from pathlib import Path, PurePosixPath

EVIDENCE = {"type": "object", "additionalProperties": False,
    "required": ["path", "start", "end", "quote"], "properties": {
        "path": {"type": "string", "minLength": 1},
        "start": {"type": "integer", "minimum": 1}, "end": {"type": "integer", "minimum": 1},
        "quote": {"type": "string", "minLength": 1, "maxLength": 1200}}}
TEXT = {"type": "string", "minLength": 1, "maxLength": 800}
SCHEMA = {"type": "object", "additionalProperties": False,
    "required": ["scope", "claims", "questions", "unknowns", "stop_reason"], "properties": {
        "scope": TEXT, "stop_reason": TEXT,
        "claims": {"type": "array", "minItems": 1, "maxItems": 16, "items": {
            "type": "object", "additionalProperties": False, "required": ["id", "text", "standing", "evidence"],
            "properties": {"id": TEXT, "text": TEXT, "standing": {"enum": ["observed", "inferred", "unknown"]},
                "evidence": {"type": "array", "maxItems": 4, "items": EVIDENCE}}}},
        "questions": {"type": "array", "maxItems": 3, "items": {"type": "object", "additionalProperties": False,
            "required": ["question", "why"], "properties": {"question": TEXT, "why": TEXT}}},
        "unknowns": {"type": "array", "maxItems": 8, "items": TEXT}}}


def structure(value, schema, where="$", errors=None):
    errors = [] if errors is None else errors
    kind = schema.get("type")
    valid = {"object": isinstance(value, dict), "array": isinstance(value, list),
             "string": isinstance(value, str), "integer": type(value) is int}.get(kind, True)
    if not valid:
        errors.append(f"{where}: expected {kind}")
        return errors
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{where}: invalid enum")
    if kind == "object":
        for field in schema.get("required", []):
            if field not in value: errors.append(f"{where}: missing {field}")
        for field, child in value.items():
            if field not in schema["properties"]: errors.append(f"{where}: unexpected {field}")
            else: structure(child, schema["properties"][field], f"{where}.{field}", errors)
    elif kind == "array":
        if not schema.get("minItems", 0) <= len(value) <= schema.get("maxItems", float("inf")):
            errors.append(f"{where}: invalid item count")
        for i, child in enumerate(value): structure(child, schema["items"], f"{where}[{i}]", errors)
    elif kind == "string":
        if not schema.get("minLength", 0) <= len(value) <= schema.get("maxLength", float("inf")):
            errors.append(f"{where}: invalid text length")
    elif kind == "integer" and value < schema.get("minimum", -float("inf")):
        errors.append(f"{where}: below minimum")
    return errors


def validate(report, root):
    errors = structure(report, SCHEMA)
    if errors: return errors
    root = Path(root).resolve()
    ids = set()
    for claim in report["claims"]:
        label = claim["id"]
        if label in ids: errors.append(f"{label}: duplicate claim id")
        ids.add(label)
        if claim["standing"] != "unknown" and not claim["evidence"]:
            errors.append(f"{label}: source/inferred claim lacks evidence")
        for cite in claim["evidence"]:
            raw = cite["path"]
            path = PurePosixPath(raw)
            if path.is_absolute() or ".." in path.parts or "\\" in raw or ":" in raw:
                errors.append(f"{label}: unsafe citation path")
                continue
            try:
                target = (root / raw).resolve(strict=True)
                target.relative_to(root)
                lines = target.read_text(encoding="utf-8").splitlines()
            except (OSError, ValueError, UnicodeError):
                errors.append(f"{label}: citation is not a readable in-root file")
                continue
            start, end = cite["start"], cite["end"]
            if not 1 <= start <= end <= len(lines) or end - start >= 12:
                errors.append(f"{label}: invalid/overlong line range")
            elif not cite["quote"].strip() or cite["quote"] not in "\n".join(lines[start - 1:end]):
                errors.append(f"{label}: quote absent from cited lines")
    return errors


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--root", type=Path, default=Path(__file__).parent / "fixture")
    args = parser.parse_args()
    errors = validate(json.loads(args.report.read_text()), args.root)
    print(json.dumps({"citation_structure_valid": not errors, "errors": errors,
                      "semantic_review": "required; no truth score inferred from valid quotes"}, indent=2))
    raise SystemExit(2 if errors else 0)
