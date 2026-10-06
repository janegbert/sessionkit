#!/usr/bin/env python3
"""Offline measurement: what deterministic rules would cut from tool results.

Walks ~/.claude/projects (main sessions first, then subagents), counts every tool result once by
its tool_use id, and weighs each saving two ways: by the characters that enter the context, and by
re-reads (characters times the API calls after the result, up to the next compaction).

Usage: shrink.py [folder-substring] [--dump-noise n]
"""
import hashlib
import json
import os
import re
import sys
from collections import defaultdict

PROJECTS = os.path.expanduser("~/.claude/projects")
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07")
DIGITS = re.compile(r"\d+")
REMINDER = re.compile(r"<system-reminder>.*?</system-reminder>", re.S)
KEEP = re.compile(r"error|fail|panic|warn|exception|traceback|denied|not found", re.I)
KNOWN = re.compile(
    r"^\s*(Compiling|Downloading|Downloaded|Checking|Fresh|Installing|Installed|Fetching|Resolving|Building|Unpacking|"
    r"Locking|Adding|Updating|Collecting|Requirement already satisfied|Using cached|Pulling|Waiting|Extracting|"
    r"Verifying|Preparing|Running)\b"
    r"|^test .* \.\.\. ok$"
    r"|^\s*(✓|✔|PASS|PASSED|ok)\b"
    r"|.*\bPASSED\b.*\[\s*\d+%\]$"
    r"|^[.\sFEsx]{20,}(\[\s*\d+%\])?$"
)
PREFIX = re.compile(r"^\s*\d+(→|\t)")
FILE = re.compile(r"[\w./~-]+\.[A-Za-z0-9]{1,6}\b")
STUB = 80  # characters a pointer to an earlier result costs
MARK = 30  # characters a "… n similar lines" marker costs
SKIP_HEAD = re.compile(r"^(cd|export|source|set|pushd|popd|true|sleep)\b|^[A-Za-z_][A-Za-z0-9_]*=")
TWO_WORDS = {"git", "cargo", "npm", "npx", "pnpm", "yarn", "docker", "gh", "sessionkit", "op-env", "php", "python", "python3", "uv", "bun", "kubectl", "make", "composer"}


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(block.get("text", "") for block in content if isinstance(block, dict) and block.get("type") == "text")
    return ""


def head_of(command):
    for part in re.split(r"&&|;|\n", command):
        part = part.strip().lstrip("(").strip()
        if not part or SKIP_HEAD.match(part):
            continue
        words = part.split()
        while words and words[0] in ("sudo", "time", "timeout", "nohup", "env", "command") or (words and re.fullmatch(r"\d+s?", words[0])):
            words = words[1:]
        if not words:
            continue
        first = os.path.basename(words[0])
        if first == "op-env" and len(words) > 2 and words[1] == "run":
            words = words[2:]
            first = os.path.basename(words[0])
        if first in TWO_WORDS and len(words) > 1 and not words[1].startswith("-"):
            return f"{first} {words[1]}"
        return first
    return "?"


def normal_command(command):
    command = re.sub(r"\s*2>&1", "", command)
    command = re.sub(r"\s*\|\s*(tail|head)\s+(-n\s*)?-?\d+\s*$", "", command.strip())
    return " ".join(command.split())


def runs(keys, least):
    """Index ranges [start, end) of at least `least` consecutive equal keys."""
    start = 0
    for i in range(1, len(keys) + 1):
        if i == len(keys) or keys[i] != keys[start]:
            if i - start >= least and keys[start] is not None:
                yield start, i
            start = i


def noise(text):
    """Characters saved per filter on one Bash result, and the masks for a dump."""
    saved = {}
    clean = ANSI.sub("", text)
    saved["ansi"] = len(text) - len(clean)
    lines = clean.split("\n")
    kept = [line.rsplit("\r", 1)[-1] if "\r" in line.rstrip("\r") else line for line in lines]
    saved["carriage"] = sum(len(a) - len(b) for a, b in zip(lines, kept))
    lines = kept
    n = len(lines)
    exact = [False] * n
    digit = [False] * n
    known = [False] * n
    cost = {"exact": 0, "digit": 0, "known": 0}
    for start, end in runs([line.strip() for line in lines], 3):
        for i in range(start + 1, end):
            exact[i] = True
        cost["exact"] += MARK if lines[start].strip() else 0
    keys = [None if (not line.strip() or KEEP.search(line)) else DIGITS.sub("0", line.strip()) for line in lines]
    for start, end in runs(keys, 4):
        for i in range(start + 1, end - 1):
            digit[i] = True
        cost["digit"] += MARK
    hits = [i for i, line in enumerate(lines) if KNOWN.match(line) and not KEEP.search(line)]
    if len(hits) >= 5:
        for i in hits:
            known[i] = True
        cost["known"] = MARK + 10
    size = lambda mask: sum(len(lines[i]) + 1 for i in range(n) if mask[i])
    saved["exact"] = max(0, size(exact) - cost["exact"])
    saved["digit"] = max(0, size(digit) - cost["digit"])
    saved["known"] = max(0, size(known) - cost["known"])
    union = [exact[i] or digit[i] or known[i] for i in range(n)]
    saved["lines"] = max(0, size(union) - cost["exact"] - cost["digit"] - cost["known"])
    saved["all"] = saved["ansi"] + saved["carriage"] + saved["lines"]
    return saved, lines, union


def transcript_files(only):
    mains, subs = [], []
    for folder in sorted(os.listdir(PROJECTS)):
        if only and only not in folder:
            continue
        path = os.path.join(PROJECTS, folder)
        if not os.path.isdir(path):
            continue
        for name in sorted(os.listdir(path)):
            if name.endswith(".jsonl"):
                mains.append(os.path.join(path, name))
                sub = os.path.join(path, name[:-6], "subagents")
                if os.path.isdir(sub):
                    subs.extend(os.path.join(sub, f) for f in sorted(os.listdir(sub)) if f.endswith(".jsonl"))
    return mains + subs


class Sums:
    def __init__(self):
        self.chars = defaultdict(float)
        self.reread = defaultdict(float)
        self.count = defaultdict(int)

    def add(self, key, chars, later, count=1):
        self.chars[key] += chars
        self.reread[key] += chars * later
        self.count[key] += count if chars > 0 else 0


def main():
    argv = sys.argv[1:]
    dump = 0
    if "--dump-noise" in argv:
        at = argv.index("--dump-noise")
        dump = int(argv[at + 1])
        del argv[at:at + 2]
    only = argv[0] if argv else ""
    seen_results = set()
    total = Sums()   # per tool: all result characters
    rule = Sums()    # per rule: characters saved
    heads = Sums()   # Bash result characters per command head
    head_noise = Sums()
    buckets = Sums()
    exts = Sums()
    head_repeat = Sums()
    end_alive = defaultdict(float)  # at a segment's end: characters alive, and of those superseded
    dumps = []
    stub_seen = 0

    for path in transcript_files(only):
        uses = {}        # tool_use id -> (name, input)
        seen_calls = set()
        calls = 0
        open_results = []  # dicts, since the last compaction
        by_command, by_output, by_file, file_lines = {}, {}, {}, defaultdict(set)
        seen_lines = set()

        def close():
            nonlocal open_results, by_command, by_output, by_file, file_lines
            for r in open_results:
                later = calls - r["before"]
                total.add(r["tool"], r["chars"], later)
                for key, value in r["saved"].items():
                    rule.add(key, value, later)
                if r["tool"] == "Bash":
                    heads.add(r["head"], r["chars"], later)
                    head_noise.add(r["head"], r["saved"].get("all", 0), later)
                    head_repeat.add(r["head"], r["saved"].get("repeat_lines:Bash", 0), later)
                if r.get("bucket"):
                    buckets.add(r["bucket"], r["chars"], later)
                if r["tool"] == "Bash" and r["head"] in ("cat", "sed", "grep", "head", "tail", "rg", "awk"):
                    exts.add(r.get("ext", "?"), r["chars"], later)
                if r["tool"] in ("Bash", "Read"):
                    end_alive[r["tool"]] += r["chars"]
                for key, at in r["superseded"].items():
                    rule.chars[key] += r["chars"]
                    rule.reread[key] += r["chars"] * (calls - at)
                    rule.count[key] += 1
                    if later > 0:
                        end_alive[key] += r["chars"]
            open_results, by_command, by_output, by_file = [], {}, {}, {}
            seen_lines.clear()
            file_lines = defaultdict(set)

        try:
            handle = open(path, "rb")
        except OSError:
            continue
        for raw in handle:
            if b'"tool_' not in raw and b'"usage"' not in raw and b"compact_boundary" not in raw:
                continue
            try:
                entry = json.loads(raw)
            except ValueError:
                continue
            if entry.get("subtype") == "compact_boundary":
                close()
                continue
            message = entry.get("message")
            if not isinstance(message, dict):
                continue
            if entry.get("type") == "assistant" and message.get("usage") and message.get("model") != "<synthetic>":
                call_id = message.get("id") or entry.get("requestId")
                if call_id is None or call_id not in seen_calls:
                    seen_calls.add(call_id)
                    calls += 1
            content = message.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                kind = block.get("type")
                if kind == "tool_use":
                    uses[block.get("id")] = (block.get("name", ""), block.get("input") or {})
                    name, tool_input = uses[block.get("id")]
                    for field in ("content", "new_string"):
                        if isinstance(tool_input.get(field), str):
                            seen_lines.update(hash(l.strip()) for l in tool_input[field].split("\n") if len(l.strip()) >= 30)
                    if name in ("Edit", "Write", "NotebookEdit"):
                        target = tool_input.get("file_path") or tool_input.get("notebook_path")
                        for r in by_file.get(target, []):
                            r["superseded"].setdefault("read_then_edited", calls)
                elif kind == "tool_result":
                    use_id = block.get("tool_use_id")
                    name, tool_input = uses.get(use_id, ("unknown", {}))
                    text = text_of(block.get("content"))
                    duplicate = use_id in seen_results
                    seen_results.add(use_id)
                    tool = name if not name.startswith("mcp__") else "mcp"
                    r = {"tool": tool, "chars": 0 if duplicate else len(text), "before": calls, "saved": {}, "superseded": {}, "head": ""}
                    open_results.append(r)
                    body = REMINDER.sub("", text)
                    digest = hashlib.md5(body.encode("utf-8", "replace")).digest()
                    if len(body) >= 500 and not duplicate:
                        if digest in by_output:
                            r["saved"]["identical_output"] = len(text) - STUB
                            r["saved"][f"identical_output:{tool if tool in ('Bash', 'Read') else 'other'}"] = len(text) - STUB
                        by_output.setdefault(digest, r)
                    if not duplicate:
                        size = len(text)
                        bucket = "<2k" if size < 2000 else "2-5k" if size < 5000 else "5-10k" if size < 10000 else "10-20k" if size < 20000 else ">20k"
                        r["bucket"] = f"{tool if tool in ('Bash', 'Read') else 'other'} {bucket}"
                        group = tool if tool in ("Bash", "Read") else "other"
                        for limit in (2000, 5000, 10000):
                            r["saved"][f"beyond_{limit // 1000}k:{group}"] = max(0, size - limit)
                        r["saved"][f"long_lines:{group}"] = sum(max(0, len(l) - 400) for l in text.split("\n"))
                        if tool == "Read":
                            r["saved"]["read_line_numbers"] = sum(len(m.group(0)) for m in map(PREFIX.match, text.split("\n")) if m)
                        if tool == "Bash":
                            target = FILE.search(tool_input.get("command", ""))
                            r["ext"] = (os.path.splitext(target.group(0))[1].lower() or "(none)") if target else "(no path)"
                    if not duplicate and tool in ("Bash", "Read", "Grep", "mcp"):
                        stripped = [PREFIX.sub("", l).strip() for l in body.split("\n")]
                        long = [(l, hash(l)) for l in stripped if len(l) >= 30]
                        again = sum(len(l) + 1 for l, h in long if h in seen_lines)
                        r["saved"][f"repeat_lines:{tool}"] = again
                        if len(body) >= 500:
                            for share in (50, 90):
                                if again >= share / 100 * len(body):
                                    r["saved"][f"repeat_{share}pct:{tool}"] = len(text) - STUB
                        seen_lines.update(h for _, h in long)
                    if name == "Bash":
                        command = tool_input.get("command", "")
                        r["head"] = head_of(command)
                        if not duplicate:
                            saved, lines, union = noise(text)
                            r["saved"].update({f"noise_{k}": v for k, v in saved.items()})
                            r["saved"]["all"] = saved["all"]
                            if dump and saved["lines"] > 2000 and len(dumps) < dump * 20:
                                dumps.append((saved["lines"], command[:200], [l for i, l in enumerate(lines) if union[i]][:8]))
                        for key, value in (("bash_same_command", command.strip()), ("bash_same_command_normal", normal_command(command))):
                            earlier = by_command.get((key, value))
                            if earlier is not None and value:
                                earlier["superseded"].setdefault(key, calls)
                            by_command[(key, value)] = r
                    elif name == "Read":
                        target = tool_input.get("file_path", "")
                        if "unchanged since" in text[:300].lower():
                            stub_seen += 1
                        whole = "offset" not in tool_input and "limit" not in tool_input
                        lines = [line for line in body.split("\n") if line.strip()]
                        known_lines = file_lines[target]
                        if not duplicate and lines:
                            again = sum(len(line) + 1 for line in lines if line in known_lines)
                            r["saved"]["read_lines_in_context"] = again
                            if again >= 0.9 * len(body) and len(body) >= 500:
                                r["saved"]["read_90pct_in_context"] = len(text) - STUB
                        known_lines.update(lines)
                        if whole:
                            for earlier in by_file.get(target, []):
                                earlier["superseded"].setdefault("read_then_whole_read", calls)
                        by_file.setdefault(target, []).append(r)
        handle.close()
        close()

    all_chars = sum(total.chars.values())
    all_reread = sum(total.reread.values())
    pct = lambda part, whole: f"{100 * part / whole:5.1f}%" if whole else "    -"
    tok = lambda chars: f"{chars / 4 / 1e6:8.2f}M"
    print(f"{sum(total.count.values()):,} tool results, {tok(all_chars).strip()} tokens, re-read {all_reread / all_chars:.0f} times over" if all_chars else "nothing")
    print(f"Read results that are already a stub of Claude Code ('unchanged since'): {stub_seen}")
    print("\ntool        results   tokens  share  re-read share")
    for name in sorted(total.chars, key=lambda k: -total.chars[k])[:8]:
        print(f"{name:10} {total.count[name]:8,} {tok(total.chars[name])} {pct(total.chars[name], all_chars)} {pct(total.reread[name], all_reread)}")

    bash_c, bash_r = total.chars["Bash"], total.reread["Bash"]
    read_c, read_r = total.chars["Read"], total.reread["Read"]
    print("\nrule                           results   tokens  of tool  of all | re-read: of tool  of all")
    rows = [
        ("noise_ansi", "Bash"), ("noise_carriage", "Bash"), ("noise_exact", "Bash"), ("noise_digit", "Bash"), ("noise_known", "Bash"),
        ("noise_lines", "Bash"), ("noise_all", "Bash"),
        ("identical_output:Bash", "Bash"), ("identical_output:Read", "Read"), ("identical_output:other", None), ("identical_output", None),
        ("read_lines_in_context", "Read"), ("read_90pct_in_context", "Read"),
        ("bash_same_command", "Bash"), ("bash_same_command_normal", "Bash"), ("read_then_whole_read", "Read"), ("read_then_edited", "Read"),
        ("repeat_lines:Bash", "Bash"), ("repeat_50pct:Bash", "Bash"), ("repeat_90pct:Bash", "Bash"),
        ("repeat_lines:Read", "Read"), ("repeat_50pct:Read", "Read"), ("repeat_90pct:Read", "Read"),
        ("repeat_lines:Grep", None), ("repeat_lines:mcp", None),
    ]
    for key, tool in rows:
        base_c, base_r = (bash_c, bash_r) if tool == "Bash" else (read_c, read_r) if tool == "Read" else (all_chars, all_reread)
        print(f"{key:30} {rule.count[key]:7,} {tok(rule.chars[key])} {pct(rule.chars[key], base_c)}  {pct(rule.chars[key], all_chars)} |"
              f"          {pct(rule.reread[key], base_r)} {pct(rule.reread[key], all_reread)}")
    print("\nsuperseded rows: tokens = size of the older results; re-read = their size times the calls after the newer result (upper bound: cutting then rewrites the cache).")
    print("\nAt the end of a segment (a compaction or the session's end), of the characters alive:")
    for key, tool in (("bash_same_command", "Bash"), ("bash_same_command_normal", "Bash"), ("read_then_whole_read", "Read"), ("read_then_edited", "Read")):
        print(f"  {key:26} {pct(end_alive[key], end_alive[tool])} of {tool}")

    print("\nceilings                       results   tokens  of tool  of all | re-read: of tool  of all")
    for group, base_c, base_r in (("Bash", bash_c, bash_r), ("Read", read_c, read_r), ("other", all_chars - bash_c - read_c, all_reread - bash_r - read_r)):
        for key in ("beyond_2k", "beyond_5k", "beyond_10k", "long_lines"):
            k = f"{key}:{group}"
            print(f"{k:30} {rule.count[k]:7,} {tok(rule.chars[k])} {pct(rule.chars[k], base_c)}  {pct(rule.chars[k], all_chars)} |          {pct(rule.reread[k], base_r)} {pct(rule.reread[k], all_reread)}")
    k = "read_line_numbers"
    print(f"{k:30} {rule.count[k]:7,} {tok(rule.chars[k])} {pct(rule.chars[k], read_c)}  {pct(rule.chars[k], all_chars)} |          {pct(rule.reread[k], read_r)} {pct(rule.reread[k], all_reread)}")
    print("\nby result size (chars)   results   tokens  share  re-read share  avg later calls")
    for name in sorted(buckets.chars):
        print(f"{name:22} {buckets.count[name]:8,} {tok(buckets.chars[name])} {pct(buckets.chars[name], all_chars)} {pct(buckets.reread[name], all_reread)}   {buckets.reread[name] / max(1, buckets.chars[name]):8.0f}")
    print("\nBash cat/sed/grep/head/tail/rg/awk by first file extension in the command")
    for name in sorted(exts.reread, key=lambda k: -exts.reread[k])[:16]:
        print(f"{name:22} {exts.count[name]:8,} {tok(exts.chars[name])} {pct(exts.chars[name], bash_c)} {pct(exts.reread[name], bash_r)}")
    print("\nBash by command head     results   tokens  share  re-read share  noise  repeat (share of head)")
    for name in sorted(heads.reread, key=lambda k: -heads.reread[k])[:30]:
        print(f"{name[:22]:22} {heads.count[name]:8,} {tok(heads.chars[name])} {pct(heads.chars[name], bash_c)} {pct(heads.reread[name], bash_r)}   {pct(head_noise.chars[name], heads.chars[name])} {pct(head_repeat.chars[name], heads.chars[name])}")
    if dump:
        print("\nLargest line-noise cuts (sample):")
        for saved, command, sample in sorted(dumps, reverse=True)[:dump]:
            print(f"\n--- {saved} chars cut | {command!r}")
            for line in sample:
                print("   ", line[:160])


main()
