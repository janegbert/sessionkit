#!/usr/bin/env python3
"""Where Claude Code's tokens go, by the choice that caused them. Offline, local, read-only.

Cost is in input-token units: input 1, cache read 0.1, cache write 1.25 (5m) or 2 (1h), output 5.
"""
import json
import os
import re
import statistics
from collections import Counter, defaultdict
from datetime import datetime

PROJECTS = os.path.expanduser("~/.claude/projects")
READ_TOOLS = {"Read", "Grep", "Glob"}
BOOKKEEPING = {"TodoWrite", "TaskCreate", "TaskUpdate", "TaskList", "TaskGet", "ToolSearch", "Skill", "EnterPlanMode", "ExitPlanMode", "ScheduleWakeup"}
WRITE_TOOLS = {"Edit", "Write", "NotebookEdit", "MultiEdit"}
BASH_READ = re.compile(r"^\s*(cd\s+\S+\s*(&&|;)\s*)?(cat|sed -n|grep|rg|ls|find|head|tail|wc|git (log|diff|status|show|branch)|tree|file|stat)\b")
PATHISH = re.compile(r"[\w.~-]*[/.][\w./~-]+")
CORRECTION = re.compile(r"^\s*(nee\b|no\b|nope|niet |wrong|fout|dat klopt niet|that'?s not|not what|stop\b|undo|revert|terug|draai)", re.I)
HOUR = 3600.0


def when(entry):
    stamp = entry.get("timestamp")
    if not stamp:
        return None
    try:
        return datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
    return ""


def files():
    mains, subs = [], []
    for folder in sorted(os.listdir(PROJECTS)):
        path = os.path.join(PROJECTS, folder)
        if not os.path.isdir(path):
            continue
        for name in sorted(os.listdir(path)):
            if name.endswith(".jsonl"):
                mains.append((os.path.join(path, name), False))
                sub = os.path.join(path, name[:-6], "subagents")
                if os.path.isdir(sub):
                    subs.extend((os.path.join(sub, f), True) for f in sorted(os.listdir(sub)) if f.endswith(".jsonl"))
    return mains + subs


def read_only(call):
    """All tools of the call only look: Read/Grep/Glob, or a Bash command that reads."""
    if not call["tools"]:
        return False
    for name, tool_input in call["tools"]:
        if name in READ_TOOLS:
            continue
        if name == "Bash" and BASH_READ.match(tool_input.get("command", "")) and ">" not in tool_input.get("command", ""):
            continue
        return False
    return True


def targets(call):
    """The names a read-only call asks for: paths and patterns."""
    out = []
    for name, tool_input in call["tools"]:
        if name == "Read":
            out.append(os.path.basename(tool_input.get("file_path", "")))
        elif name in ("Grep", "Glob"):
            out.append(str(tool_input.get("pattern", "")))
        elif name == "Bash":
            out.extend(os.path.basename(p.rstrip("/")) for p in PATHISH.findall(tool_input.get("command", "")))
    return [t for t in out if len(t) >= 4]


G = defaultdict(float)      # named sums
C = Counter()               # named counts
sole = defaultdict(lambda: [0, 0.0])
errors = defaultdict(lambda: [0, 0.0])
error_lines = Counter()
short_prompts = Counter()
gap_rebuild = defaultdict(lambda: [0, 0.0])
keepalive = defaultdict(float)
prefixes = {"main": [], "sub": []}
sub_sizes = Counter()
prompt_ctx = []
seen_call_ids = set()


def process(path, is_sub):
    kind = "sub" if is_sub else "main"
    calls, by_id = [], {}
    results = []            # (chars, before, turn, segment, tool, path)
    uses = {}               # tool_use id -> (name, input, call)
    turns = [{"human": None, "first": None, "last": None, "calls": []}]
    segment, seg_end = 0, {}
    path_turns = defaultdict(set)  # file path -> turns in which a tool touched it
    after_compaction = False
    last_assistant_text = ""
    try:
        handle = open(path, "rb")
    except OSError:
        return
    for raw in handle:
        try:
            entry = json.loads(raw)
        except ValueError:
            continue
        if entry.get("subtype") == "compact_boundary":
            seg_end[segment] = len(calls)
            segment += 1
            after_compaction = True
            continue
        message = entry.get("message")
        if not isinstance(message, dict):
            continue
        kind_of = entry.get("type")
        content = message.get("content")
        if kind_of == "assistant":
            usage = message.get("usage")
            if not usage or message.get("model") == "<synthetic>":
                continue
            call_id = message.get("id") or entry.get("requestId") or f"{path}:{len(calls)}"
            call = by_id.get(call_id)
            if call is None:
                if call_id in seen_call_ids:
                    call = {"skip": True, "tools": [], "text": ""}
                    by_id[call_id] = call
                else:
                    seen_call_ids.add(call_id)
                    split = usage.get("cache_creation") or {}
                    write = usage.get("cache_creation_input_tokens") or 0
                    w1h = split.get("ephemeral_1h_input_tokens") or 0
                    w5m = split.get("ephemeral_5m_input_tokens")
                    w5m = write - w1h if w5m is None else w5m
                    read = usage.get("cache_read_input_tokens") or 0
                    fresh = usage.get("input_tokens") or 0
                    out = usage.get("output_tokens") or 0
                    call = {"skip": False, "tools": [], "text": "", "time": when(entry), "ctx": fresh + read + write, "read": read,
                            "w5m": w5m, "w1h": w1h, "out": out, "units": fresh + 0.1 * read + 1.25 * w5m + 2 * w1h + 5 * out,
                            "turn": len(turns) - 1, "segment": segment, "index": len(calls), "after_compaction": after_compaction,
                            "result_text": ""}
                    after_compaction = False
                    by_id[call_id] = call
                    calls.append(call)
                    turns[-1]["calls"].append(call)
            elif not call["skip"] and (usage.get("output_tokens") or 0) > call["out"]:
                # a call is written as a line per block; an early line can carry the output count of the stream's start
                call["units"] += 5 * ((usage.get("output_tokens") or 0) - call["out"])
                call["out"] = usage.get("output_tokens") or 0
            if isinstance(content, list):
                for block in content:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "tool_use":
                        tool_input = block.get("input") or {}
                        call["tools"].append((block.get("name", ""), tool_input))
                        uses[block.get("id")] = (block.get("name", ""), tool_input, call)
                        target = tool_input.get("file_path")
                        if target:
                            path_turns[target].add(len(turns) - 1)
                    elif block.get("type") == "text":
                        call["text"] += block.get("text", "")
                        last_assistant_text = call["text"]
        elif kind_of == "user":
            has_result = False
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_result":
                        has_result = True
                        name, tool_input, call = uses.get(block.get("tool_use_id"), ("unknown", {}, None))
                        text = text_of(block.get("content"))
                        if call is not None and not call.get("skip"):
                            call["result_text"] += text[:20000]
                            results.append((len(text), len(calls), len(turns) - 1, segment, name, tool_input.get("file_path")))
                            if block.get("is_error"):
                                errors[name][0] += 1
                                errors[name][1] += call["ctx"]
                                error_lines[f"{name}: {re.sub(r'[0-9]+', 'N', text.strip()[:70])}"] += 1
            if has_result or entry.get("isMeta") or entry.get("isCompactSummary") or entry.get("isSidechain") and is_sub and turns[-1]["human"] is not None:
                continue
            text = text_of(content).strip()
            if not text:
                continue
            if "[Request interrupted by user" in text:
                C["interrupted turns"] += 1
                G["interrupted turn units"] += sum(c["units"] for c in turns[-1]["calls"])
                continue
            if text.startswith("<"):
                if text.startswith("<task-notification") or text.startswith("<system-reminder"):
                    turns.append({"human": None, "first": None, "last": None, "calls": [], "auto": True})
                continue
            turns.append({"human": text, "calls": [], "asked": last_assistant_text.rstrip().endswith("?")})
    handle.close()
    seg_end[segment] = len(calls)
    if not calls:
        return

    # totals
    for call in calls:
        G[f"{kind} units"] += call["units"]
        G[f"{kind} read units"] += 0.1 * call["read"]
        G[f"{kind} write units"] += 1.25 * call["w5m"] + 2 * call["w1h"]
        G[f"{kind} output units"] += 5 * call["out"]
        G[f"{kind} ctx"] += call["ctx"]
        G["w5m"] += call["w5m"]
        G["w1h"] += call["w1h"]
        C[f"{kind} calls"] += 1
        G[f"{kind} ctx above 200K"] += max(0, call["ctx"] - 200_000)
        if call["ctx"] > 200_000:
            G[f"{kind} units in calls above 200K"] += call["units"]
        names = [name for name, _ in call["tools"]]
        if not names:
            key = "(text only)"
        elif len(names) == 1:
            key = names[0] if not names[0].startswith("mcp__") else "mcp"
        else:
            key = "(2+ tools)"
        sole[f"{kind} {key}"][0] += 1
        sole[f"{kind} {key}"][1] += call["ctx"]
        if names and set(names) <= BOOKKEEPING:
            C[f"{kind} bookkeeping-only calls"] += 1
            G[f"{kind} bookkeeping-only ctx"] += call["ctx"]
        C[f"{kind} tool uses"] += len(names)

    # prefix: what the first call already carries
    first = calls[0]["ctx"]
    if first < 80_000:
        prefixes[kind].append(first)
        G[f"{kind} prefix ctx"] += first * len(calls)
    else:
        C[f"{kind} transcripts starting above 80K"] += 1
        G[f"{kind} ctx of transcripts starting above 80K"] += sum(c["ctx"] for c in calls)
    if is_sub:
        size = "1-3 calls" if len(calls) <= 3 else "4-10 calls" if len(calls) <= 10 else "11-30 calls" if len(calls) <= 30 else "31+ calls"
        sub_sizes[size] += 1
        G[f"sub units {size}"] += sum(c["units"] for c in calls)
        G["sub first-call units"] += calls[0]["units"]

    # batchable reads: the next read-only call asks for nothing the previous results named
    for a, b in zip(calls, calls[1:]):
        if a["turn"] != b["turn"] or not read_only(a) or not read_only(b):
            continue
        C[f"{kind} read-only call after read-only call"] += 1
        G[f"{kind} read-after-read ctx"] += b["ctx"]
        wanted = targets(b)
        if wanted and not any(t in a["result_text"] for t in wanted):
            C[f"{kind} batchable read calls"] += 1
            G[f"{kind} batchable ctx"] += b["ctx"]

    # cache rebuilds by pause, and what keeping warm would do (main sessions)
    for a, b in zip(calls, calls[1:]):
        if a["time"] is None or b["time"] is None:
            continue
        gap = b["time"] - a["time"]
        created = b["w5m"] + b["w1h"]
        rebuilt = created > 0.5 * b["ctx"] and b["ctx"] > 30_000 and not b["after_compaction"]
        if rebuilt:
            bucket = "<5m" if gap < 300 else "5-60m" if gap < HOUR else "1-2h" if gap < 2 * HOUR else "2-4h" if gap < 4 * HOUR else "4-8h" if gap < 8 * HOUR else "8-24h" if gap < 24 * HOUR else ">24h"
            gap_rebuild[f"{kind} {bucket}"][0] += 1
            gap_rebuild[f"{kind} {bucket}"][1] += 1.25 * b["w5m"] + 2 * b["w1h"]
        if not is_sub and gap > HOUR:
            for hours in (2, 4, 8, 16, 24):
                pings = int(min(gap, hours * HOUR) / (55 * 60))
                keepalive[f"{hours}h ping units"] += pings * 0.1 * a["ctx"]
                if gap <= hours * HOUR and rebuilt:
                    keepalive[f"{hours}h saved units"] += 1.25 * b["w5m"] + 2 * b["w1h"] - 0.1 * created
    if not is_sub:
        for hours in (2, 4, 8, 16, 24):  # the last pause never ends: every ping is lost
            keepalive[f"{hours}h ping units"] += int(hours * HOUR / (55 * 60)) * 0.1 * calls[-1]["ctx"]

    if is_sub:
        return

    # turns
    turn_last = {}
    for index, turn in enumerate(turns):
        if turn["calls"]:
            turn_last[index] = turn["calls"][-1]["index"] + 1
    for index, turn in enumerate(turns):
        human = turn.get("human")
        tcalls = turn["calls"]
        if human is None or not tcalls:
            continue
        C["human prompts"] += 1
        prompt_ctx.append(tcalls[0]["ctx"])
        units = sum(c["units"] for c in tcalls)
        if len(human) <= 25 and len(human.split()) <= 4:
            C["short prompts"] += 1
            G["short prompt first-call ctx"] += tcalls[0]["ctx"]
            short_prompts[human.lower()] += 1
            if turn.get("asked"):
                C["short prompts after a question of the agent"] += 1
                G["short-after-question first-call units"] += tcalls[0]["units"]
        if len(tcalls) == 1 and not tcalls[0]["tools"] and tcalls[0]["text"].rstrip().endswith("?"):
            C["turns that only ask back"] += 1
            G["ask-back units"] += units
        if len(tcalls) == 1 and not tcalls[0]["tools"]:
            C["turns answered without a tool"] += 1
            G["no-tool turn units"] += units
            if tcalls[0]["ctx"] > 150_000:
                C["no-tool turns above 150K"] += 1
                G["no-tool turn units above 150K"] += units
        # exploration before the first change
        for call in tcalls:
            if not read_only(call):
                break
            C["exploration calls at a turn's start"] += 1
            G["exploration ctx at a turn's start"] += call["ctx"]
        if CORRECTION.match(human) and index > 0:
            C["correction prompts"] += 1
            previous = index - 1
            for chars, before, turn_of, seg, _, _ in results:
                if turn_of == previous:
                    G["dead branch re-read chars"] += chars * max(0, seg_end.get(seg, len(calls)) - max(before, turn_last.get(previous, before)))

    # turn folding: a result is read again inside its turn, and after its turn
    for chars, before, turn_of, seg, name, target in results:
        end = seg_end.get(seg, len(calls))
        last = min(max(turn_last.get(turn_of, before), before), end)
        G["result re-read chars in the same turn"] += chars * (last - before)
        later = chars * (end - last)
        G["result re-read chars in later turns"] += later
        if name == "Read" and target:
            G["Read re-read chars in later turns"] += later
            if any(t > turn_of for t in path_turns.get(target, ())):
                G["Read re-read chars in later turns, file touched again"] += later
    G["assistant text chars"] += sum(len(c["text"]) for c in calls)


def main():
    for path, is_sub in files():
        process(path, is_sub)
    units = G["main units"] + G["sub units"]
    pct = lambda part, whole=None: f"{100 * part / (whole or units):5.1f}%"
    m = lambda value: f"{value / 1e6:9.1f}M"
    print(f"cost in input-token units: {m(units)}  main {pct(G['main units'])}  subagents {pct(G['sub units'])}")
    for kind in ("main", "sub"):
        print(f"  {kind}: calls {C[kind + ' calls']:,}  tool uses per call {C[kind + ' tool uses'] / max(1, C[kind + ' calls']):.2f}  "
              f"reads {pct(G[kind + ' read units'])}  writes {pct(G[kind + ' write units'])}  output {pct(G[kind + ' output units'])}  avg ctx {G[kind + ' ctx'] / max(1, C[kind + ' calls']) / 1000:.0f}K")
    print(f"cache writes: 1h {m(G['w1h'])} tokens, 5m {m(G['w5m'])} tokens")
    all_ctx = G["main ctx"] + G["sub ctx"]

    print("\n1. fixed prefix (context of a transcript's first call, times its calls)")
    for kind in ("main", "sub"):
        values = prefixes[kind]
        if values:
            print(f"  {kind}: median first call {statistics.median(values) / 1000:.0f}K (p25 {statistics.quantiles(values, n=4)[0] / 1000:.0f}K, p75 {statistics.quantiles(values, n=4)[2] / 1000:.0f}K), "
                  f"{pct(G[kind + ' prefix ctx'], G[kind + ' ctx'])} of {kind} context read; of all context {pct(G[kind + ' prefix ctx'], all_ctx)}; "
                  f"transcripts starting above 80K: {C[kind + ' transcripts starting above 80K']} with {pct(G[kind + ' ctx of transcripts starting above 80K'], G[kind + ' ctx'])} of {kind} context")

    print("\n2. calls by their tool (share of context read)")
    for kind, total in (("main", G["main ctx"]), ("sub", G["sub ctx"])):
        rows = sorted(((k, v) for k, v in sole.items() if k.startswith(kind + " ")), key=lambda kv: -kv[1][1])[:14]
        print("  " + "; ".join(f"{k.split(' ', 1)[1]} {v[0]:,} ({pct(v[1], total).strip()})" for k, v in rows))
        print(f"  {kind} bookkeeping-only calls: {C[kind + ' bookkeeping-only calls']:,}, {pct(G[kind + ' bookkeeping-only ctx'], total)} of {kind} context")

    print("\n3. read-only call after read-only call in one turn")
    for kind in ("main", "sub"):
        print(f"  {kind}: {C[kind + ' read-only call after read-only call']:,} calls, {pct(G[kind + ' read-after-read ctx'], G[kind + ' ctx'])} of context; "
              f"asked for nothing the previous results named: {C[kind + ' batchable read calls']:,} calls, {pct(G[kind + ' batchable ctx'], G[kind + ' ctx'])}")

    print("\n4. tool results: when are they read again (main sessions)")
    same, later = G["result re-read chars in the same turn"], G["result re-read chars in later turns"]
    print(f"  same turn {pct(same, same + later)}, later turns {pct(later, same + later)}; later-turn re-reads cost {m(later / 4 * 0.1)} units = {pct(later / 4 * 0.1)} of all cost")
    print(f"  of the Read part read again in later turns, {pct(G['Read re-read chars in later turns, file touched again'], G['Read re-read chars in later turns'])} is of files a later turn touched again")

    print("\n5. prompts (main sessions)")
    print(f"  human prompts {C['human prompts']:,}; median context at a prompt {statistics.median(prompt_ctx) / 1000:.0f}K")
    print(f"  short prompts (<=4 words) {C['short prompts']:,}, avg context {G['short prompt first-call ctx'] / max(1, C['short prompts']) / 1000:.0f}K; after a question of the agent: {C['short prompts after a question of the agent']:,}, first call {pct(G['short-after-question first-call units'])} of cost")
    print("  most common: " + ", ".join(f"{k!r} {v}" for k, v in short_prompts.most_common(14)))
    print(f"  turns that only ask back {C['turns that only ask back']:,} ({pct(G['ask-back units'])} of cost); turns answered without a tool {C['turns answered without a tool']:,} ({pct(G['no-tool turn units'])}), above 150K: {C['no-tool turns above 150K']:,} ({pct(G['no-tool turn units above 150K'])})")
    print(f"  interrupted turns {C['interrupted turns']:,} ({pct(G['interrupted turn units'])} of cost spent in them)")
    print(f"  correction prompts {C['correction prompts']:,}; re-reads of the rejected turn's results afterwards: {pct(G['dead branch re-read chars'] / 4 * 0.1)} of cost")
    print(f"  exploration calls at a turn's start {C['exploration calls at a turn' + chr(39) + 's start']:,}, {pct(G['exploration ctx at a turn' + chr(39) + 's start'], G['main ctx'])} of main context read")

    print("\n6. errors by tool (count, share of all calls' context)")
    print("  " + "; ".join(f"{k} {v[0]:,} ({pct(v[1], all_ctx).strip()})" for k, v in sorted(errors.items(), key=lambda kv: -kv[1][0])[:10]))
    print(f"  total {sum(v[0] for v in errors.values()):,} errors, {pct(sum(v[1] for v in errors.values()), all_ctx)} of context")
    for line, count in error_lines.most_common(14):
        print(f"    {count:5}  {line}")

    print("\n7. cache writes of more than half the context, by the pause before (count, share of cost)")
    order = ["<5m", "5-60m", "1-2h", "2-4h", "4-8h", "8-24h", ">24h"]
    for kind in ("main", "sub"):
        print(f"  {kind}: " + "; ".join(f"{b} {gap_rebuild[kind + ' ' + b][0]:,} ({pct(gap_rebuild[kind + ' ' + b][1]).strip()})" for b in order))
    print("  keeping a main session warm with a ping every 55 minutes, for at most H hours:")
    for hours in (2, 4, 8, 16, 24):
        saved, pings = keepalive[f"{hours}h saved units"], keepalive[f"{hours}h ping units"]
        print(f"    H={hours:2}h  saved {pct(saved)}  pings {pct(pings)}  net {pct(saved - pings)}")

    print("\n8. context above 200K")
    for kind in ("main", "sub"):
        print(f"  {kind}: {pct(G[kind + ' ctx above 200K'], G[kind + ' ctx'])} of context read lies above 200K; calls above 200K carry {pct(G[kind + ' units in calls above 200K'], G[kind + ' units'])} of {kind} cost")

    print("\n9. subagents by size")
    for size in ("1-3 calls", "4-10 calls", "11-30 calls", "31+ calls"):
        print(f"  {size}: {sub_sizes[size]:,} subagents, {pct(G['sub units ' + size], G['sub units'])} of subagent cost")
    print(f"  first calls: {pct(G['sub first-call units'], G['sub units'])} of subagent cost")


main()
