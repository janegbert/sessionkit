"""Local eval-only CLI proxy. Only synthetic-root grep/ask; no model cost guesses."""
import json
import re
import subprocess
import sys
import time
from pathlib import Path


def target(argv, root):
    if not argv or argv[0] not in ["grep", "ask"]: raise ValueError("only grep/ask permitted")
    if argv[0] == "grep":
        # Exact argv emitted by the production native tool, not arbitrary CLI flags.
        if len(argv) != 9 or argv[1:3] != ["--agent-view", "--max-files"] or argv[4] != "--max-source-bytes" or argv[6] != "--":
            raise ValueError("unexpected grep options")
        if not 1 <= int(argv[3]) <= 100 or not 1 <= int(argv[5]) <= 100000:
            raise ValueError("invalid source/file budget")
        raw = argv[8]
    else:
        index = 2 if len(argv) > 1 and argv[1] == "--find" else 1
        count = len(argv) - index - 1
        if not 1 <= count <= 10 or (index == 2 and count != 1):
            raise ValueError("invalid question count")
        raw = argv[index]
        if raw.startswith("-") or any(q.startswith("-") for q in argv[index + 1:]):
            raise ValueError("unsafe ask options")
    root = Path(root).resolve()
    path = Path(raw)
    path = (path if path.is_absolute() else root / path).resolve(strict=True)
    path.relative_to(root)
    if argv[0] == "grep" and not path.is_dir(): raise ValueError("grep requires directory")
    if argv[0] == "ask" and not path.is_file(): raise ValueError("ask requires file")
    return path


def reported_cost(operation, stdout, exit_code):
    if operation != "ask" or exit_code != 0: return None
    # Only parse the CLI's summary line, never a $ literal in retrieved source.
    lines = [line for line in stdout.splitlines() if line.strip()]
    if not lines: return None
    match = re.search(r" tokens · \$(\d+(?:\.\d+)?) · ", lines[-1])
    return float(match.group(1)) if match else None


def append(path, event):
    with Path(path).open("a") as out: out.write(json.dumps(event) + "\n")


def main(binary, root, journal):
    argv = sys.argv[1:]
    operation = argv[0] if argv else "unknown"
    try: path = target(argv, root)
    except (ValueError, OSError) as error:
        append(journal, {"operation": operation, "denied": True, "cli_started": False, "cost_estimate_usd": 0.0})
        print(f"sessionkit eval: source root restriction: {error}")
        raise SystemExit(1)
    call = f"{time.time_ns()}-{operation}"
    append(journal, {"id": call, "operation": operation, "phase": "start", "cli_started": True,
                     "target": str(path.relative_to(Path(root).resolve()))})
    started = time.monotonic()
    try:
        completed = subprocess.run([binary, *argv], cwd=root, capture_output=True, text=True, timeout=115)
        append(journal, {"id": call, "operation": operation, "phase": "end", "exit_code": completed.returncode,
                         "seconds": time.monotonic() - started,
                         "cost_estimate_usd": reported_cost(operation, completed.stdout, completed.returncode)})
        sys.stdout.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise SystemExit(completed.returncode)
    except (subprocess.TimeoutExpired, OSError):
        append(journal, {"id": call, "operation": operation, "phase": "end", "exit_code": None,
                         "cost_estimate_usd": None})
        print("sessionkit eval: CLI failed or timed out; incurred cost is unknown")
        raise SystemExit(1)


def costs(journal):
    events = [json.loads(line) for line in Path(journal).read_text().splitlines()] if Path(journal).exists() else []
    starts = [event for event in events if event.get("phase") == "start"]
    ends = {event["id"]: event for event in events if event.get("phase") == "end"}
    values = [ends.get(event["id"], {}).get("cost_estimate_usd") for event in starts]
    return {"cli_calls": len(starts), "grep_calls": sum(e["operation"] == "grep" for e in starts),
            "ask_calls": sum(e["operation"] == "ask" for e in starts),
            "denied_calls": sum(e.get("denied", False) for e in events),
            "known_cli_reported_estimate_usd": sum(value for value in values if value is not None),
            "jev_cost_estimate_usd": sum(values) if all(value is not None for value in values) else None,
            "cost_basis": "ask: rounded CLI usage-based estimate; grep/failures: unknown; not billing"}
