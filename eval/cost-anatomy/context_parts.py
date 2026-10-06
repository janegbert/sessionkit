#!/usr/bin/env python3
"""What the cached context consists of: every block times the API calls after it (to the next compaction)."""
import json, os, re, statistics
from collections import Counter, defaultdict
from datetime import datetime
PROJECTS = os.path.expanduser("~/.claude/projects")
R = {"main": defaultdict(float), "sub": defaultdict(float)}     # category -> token-reads
ACTUAL = defaultdict(float)
REB = Counter(); REBU = defaultdict(float)
TYPES = Counter(); ATT = defaultdict(float)
META = Counter()

def text_of(content):
    if isinstance(content, str): return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
    return ""

def when(e):
    try: return datetime.fromisoformat(e["timestamp"].replace("Z", "+00:00")).timestamp()
    except Exception: return None

def files():
    mains, subs = [], []
    for folder in sorted(os.listdir(PROJECTS)):
        path = os.path.join(PROJECTS, folder)
        if not os.path.isdir(path): continue
        for name in sorted(os.listdir(path)):
            if name.endswith(".jsonl"):
                mains.append((os.path.join(path, name), "main"))
                sub = os.path.join(path, name[:-6], "subagents")
                if os.path.isdir(sub):
                    subs.extend((os.path.join(sub, f), "sub") for f in sorted(os.listdir(sub)) if f.endswith(".jsonl"))
    return mains + subs

seen = set()
def process(path, kind):
    items = []   # (category, tokens, calls_before)
    calls = 0; ids = {}; first_ctx = None; prev = None; uses = {}; after_compaction = False
    def close():
        nonlocal items
        for cat, tokens, before in items:
            R[kind][cat] += tokens * (calls - before)
        if first_ctx is not None:
            pass
        items = []
    seg_start = 0
    for raw in open(path, "rb"):
        try: e = json.loads(raw)
        except ValueError: continue
        t = e.get("type")
        if e.get("subtype") == "compact_boundary":
            close(); after_compaction = True
            R[kind]["prefix"] += 0
            continue
        if t == "attachment":
            a = e.get("attachment") or {}
            size = len(json.dumps(a)) / 4
            items.append((f"attachment: {a.get('type', '?')}", size, calls))
            continue
        m = e.get("message")
        if not isinstance(m, dict): continue
        content = m.get("content")
        if t == "assistant":
            u = m.get("usage")
            if not u or m.get("model") == "<synthetic>": continue
            cid = m.get("id") or e.get("requestId")
            new = cid not in ids
            if new:
                if cid in seen: ids[cid] = None
                else:
                    seen.add(cid); ids[cid] = [u.get("output_tokens") or 0]
                    read = u.get("cache_read_input_tokens") or 0; write = u.get("cache_creation_input_tokens") or 0
                    ctx = (u.get("input_tokens") or 0) + read + write
                    if first_ctx is None: first_ctx = min(ctx, 80_000)
                    ACTUAL[kind + " ctx"] += ctx; ACTUAL[kind + " calls"] += 1
                    R[kind]["prefix (first call of the transcript)"] += first_ctx
                    now = when(e)
                    split = u.get("cache_creation") or {}
                    w1h = split.get("ephemeral_1h_input_tokens") or 0
                    if prev and now and prev["time"] and write > 0.5 * ctx and ctx > 30_000 and not after_compaction:
                        gap = now - prev["time"]
                        b = "<5m" if gap < 300 else "5-60m" if gap < 3600 else ">1h"
                        shape = "shrunk" if ctx < 0.9 * prev["ctx"] else "same size"
                        ttl = "prev wrote 1h" if prev["w1h"] > 0 else "prev wrote 5m" if prev["w"] > 0 else "prev wrote nothing"
                        model = "model changed" if prev["model"] != m.get("model") else "same model"
                        key = f"{kind} {b} | {shape} | {ttl} | {model}"
                        REB[key] += 1; REBU[key] += 1.25 * (write - w1h) + 2 * w1h
                    after_compaction = False
                    prev = {"time": now, "ctx": ctx, "w1h": w1h, "w": write, "model": m.get("model")}
                    items.append(("assistant output (text, thinking, tool inputs)", u.get("output_tokens") or 0, calls + 1))
                    calls += 1
            if not new and ids.get(cid) and (u.get("output_tokens") or 0) > ids[cid][0]:
                # a later line of the same call carries the final output count
                items.append(("assistant output (text, thinking, tool inputs)", (u.get("output_tokens") or 0) - ids[cid][0], calls))
                ids[cid][0] = u.get("output_tokens") or 0
            if ids.get(cid) and isinstance(content, list):
                for b in content:
                    if not isinstance(b, dict): continue
                    bt = b.get("type")
                    if bt == "text": items.append(("  of which text", len(b.get("text", "")) / 4, calls))
                    elif bt == "thinking": items.append(("  of which thinking (shown text)", len(b.get("thinking", "")) / 4, calls))
                    elif bt == "tool_use":
                        name = b.get("name", "")
                        uses[b.get("id")] = name
                        label = name if name in ("Write", "Edit", "Bash", "Agent", "SendMessage") else "mcp" if name.startswith("mcp__") else "other"
                        items.append((f"  of which input of {label}", len(json.dumps(b.get("input") or {})) / 4, calls))
        elif t == "user":
            if isinstance(content, list):
                for b in content:
                    if not isinstance(b, dict): continue
                    if b.get("type") == "tool_result":
                        c = b.get("content")
                        items.append(("tool results (text)", len(text_of(c)) / 4, calls))
                        if isinstance(c, list):
                            n = sum(1 for x in c if isinstance(x, dict) and x.get("type") == "image")
                            if n: items.append(("tool results (images, 1.5K each)", 1500 * n, calls))
                    elif b.get("type") == "image":
                        items.append(("user images (1.5K each)", 1500, calls))
            text = text_of(content) if not (isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content)) else \
                "\n".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
            if text:
                s = text.lstrip()
                if e.get("isCompactSummary"): cat = "user: compaction summary"
                elif s.startswith("<teammate-message") or "teammate_id" in s[:200]: cat = "user: teammate message"
                elif s.startswith("<task-notification"): cat = "user: task notification"
                elif s.startswith("<system-reminder"): cat = "user: system reminder"
                elif s.startswith("<command-") or s.startswith("<local-command"): cat = "user: command and its output"
                elif e.get("isMeta"): cat = "user: meta (skill bodies and the like)"
                else: cat = "user: prompt"
                items.append((cat, len(text) / 4, calls))
    close()

for path, kind in files():
    try: process(path, kind)
    except OSError: pass
for kind in ("main", "sub"):
    total = ACTUAL[kind + " ctx"]
    print(f"\n{kind}: {ACTUAL[kind + ' calls']:,.0f} calls read {total / 1e9:.2f}B tokens of context")
    # hook_success and prompt_snapshot are log entries, not context
    top = [(k, v) for k, v in R[kind].items() if not k.startswith("  ") and k not in ("attachment: hook_success", "attachment: prompt_snapshot")]
    covered = sum(v for _, v in top)
    for k, v in sorted(top, key=lambda kv: -kv[1])[:22]:
        print(f"  {100 * v / total:5.1f}%  {k}")
        if k.startswith("assistant output"):
            for k2, v2 in sorted(((k2, v2) for k2, v2 in R[kind].items() if k2.startswith("  ")), key=lambda kv: -kv[1]):
                print(f"         {100 * v2 / total:5.1f}% {k2}")
    print(f"  {100 * covered / total:5.1f}%  accounted for")
print("\ncache writes of more than half the context (count, cost units in M)")
for k, n in sorted(REB.items(), key=lambda kv: -REBU[kv[0]])[:16]:
    print(f"  {n:5}  {REBU[k] / 1e6:7.1f}M  {k}")
