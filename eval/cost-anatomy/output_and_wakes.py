import json, os, statistics
from collections import Counter, defaultdict
from datetime import datetime
PROJECTS = os.path.expanduser("~/.claude/projects")
def when(e):
    try: return datetime.fromisoformat(e["timestamp"].replace("Z", "+00:00")).timestamp()
    except Exception: return None
def text_of(c):
    if isinstance(c, str): return c
    if isinstance(c, list): return "\n".join(b.get("text", "") for b in c if isinstance(b, dict) and b.get("type") == "text")
    return ""
def files():
    out = []
    for folder in sorted(os.listdir(PROJECTS)):
        path = os.path.join(PROJECTS, folder)
        if not os.path.isdir(path): continue
        for name in sorted(os.listdir(path)):
            if name.endswith(".jsonl"):
                out.append((os.path.join(path, name), "main"))
                sub = os.path.join(path, name[:-6], "subagents")
                if os.path.isdir(sub):
                    out.extend((os.path.join(sub, f), "sub") for f in sorted(os.listdir(sub)) if f.endswith(".jsonl"))
    return sorted(out, key=lambda x: x[1])
S = defaultdict(float); N = Counter(); hook = defaultdict(float); hookn = Counter(); wake = Counter(); wakeu = defaultdict(float)
agent_types = Counter(); agent_units = defaultdict(float); sample = {}
gaps = Counter(); cross = []
seen = set()
for path, kind in files():
    calls = []; ids = {}; turn = 0; pending_user = 0.0
    meta = None
    if kind == "sub":
        mp = path[:-6] + ".meta.json"
        if os.path.exists(mp):
            try: meta = json.load(open(mp))
            except Exception: meta = None
    try: handle = open(path, "rb")
    except OSError: continue
    for raw in handle:
        try: e = json.loads(raw)
        except ValueError: continue
        t = e.get("type")
        if e.get("subtype") == "compact_boundary":
            if calls: calls[-1]["compacted_after"] = True
            continue
        if t == "attachment":
            a = e.get("attachment") or {}
            if a.get("type") == "hook_success":
                key = f"{a.get('hookEvent')} {str(a.get('hookName'))[:40]}"
                hook[key] += len(json.dumps(a)); hookn[key] += 1
                sample.setdefault(key, {k: (str(v)[:80]) for k, v in a.items()})
            continue
        m = e.get("message")
        if not isinstance(m, dict): continue
        c = m.get("content")
        if t == "assistant":
            u = m.get("usage")
            if not u or m.get("model") == "<synthetic>": continue
            cid = m.get("id") or e.get("requestId")
            call = ids.get(cid)
            if call is None:
                if cid in seen: ids[cid] = {"skip": True, "out": 0, "first_out": 0, "visible": 0, "tools": []}; continue
                seen.add(cid)
                read = u.get("cache_read_input_tokens") or 0; write = u.get("cache_creation_input_tokens") or 0
                w1h = (u.get("cache_creation") or {}).get("ephemeral_1h_input_tokens") or 0
                call = {"skip": False, "ctx": (u.get("input_tokens") or 0) + read + write, "write": write, "w1h": w1h, "time": when(e), "out": 0,
                        "first_out": u.get("output_tokens") or 0, "visible": 0.0, "tools": [], "turn": turn, "added": 0.0}
                ids[cid] = call; calls.append(call)
            call["out"] = max(call["out"], u.get("output_tokens") or 0)
            if isinstance(c, list):
                for b in c:
                    if not isinstance(b, dict): continue
                    if b.get("type") == "text": call["visible"] += len(b.get("text", "")) / 4
                    elif b.get("type") == "tool_use":
                        call["visible"] += len(json.dumps(b.get("input") or {})) / 4
                        call["tools"].append((b.get("name"), b.get("input") or {}))
        elif t == "user" and calls:
            is_result = isinstance(c, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in c)
            size = 0.0
            if isinstance(c, list):
                for b in c:
                    if isinstance(b, dict) and b.get("type") == "tool_result": size += len(text_of(b.get("content"))) / 4
            size += len(text_of(c)) / 4
            calls[-1]["added"] += size
            if not is_result and not e.get("isMeta"): turn += 1
    handle.close()
    calls = [c for c in calls if not c["skip"]]
    if not calls: continue
    if kind == "sub":
        at = (meta or {}).get("agentType") or "?"
        agent_types[at] += 1
    for i, call in enumerate(calls):
        S[kind + " out first"] += call["first_out"]; S[kind + " out max"] += call["out"]; S[kind + " visible"] += call["visible"]
        later = 0
        for j in range(i + 1, len(calls)):
            later += 1
            if calls[j - 1].get("compacted_after"): later -= 1; break
        S[kind + " out re-read"] += call["out"] * later
        S[kind + " visible re-read"] += call["visible"] * later
        S[kind + " ctx"] += call["ctx"]
        if kind == "sub":
            agent_units[(meta or {}).get("agentType") or "?"] += call["ctx"]
    for a, b in zip(calls, calls[1:]):
        if a.get("compacted_after") or a["time"] is None or b["time"] is None: continue
        if b["turn"] != a["turn"]:
            # across a turn: growth of the context against what the last call put out
            cross.append((b["ctx"] - a["ctx"], a["out"], a["visible"], a["added"]))
        gap = b["time"] - a["time"]
        if kind == "sub" and b["write"] > 0.5 * b["ctx"] and b["ctx"] > 30_000 and gap > 200:
            bucket = "3-5m" if gap < 300 else "5-10m" if gap < 600 else "10-30m" if gap < 1800 else "30-60m" if gap < 3600 else ">1h"
            names = [n for n, _ in a["tools"]] or ["(text only: waited for a message)"]
            first = names[0]
            if first == "Bash":
                inp = a["tools"][0][1]
                first = "Bash in background" if inp.get("run_in_background") else "Bash (foreground)"
            units = 1.25 * (b["write"] - b["w1h"]) + 2 * b["w1h"]
            wake[first] += 1; wakeu[first] += units; gaps[bucket] += 1; S["gapu " + bucket] += units
for kind in ("main", "sub"):
    print(f"{kind}: output tokens first line {S[kind+' out first']/1e6:.1f}M, max per call {S[kind+' out max']/1e6:.1f}M, visible text+inputs {S[kind+' visible']/1e6:.1f}M; "
          f"re-read share of context: output {100*S[kind+' out re-read']/S[kind+' ctx']:.1f}%, visible {100*S[kind+' visible re-read']/S[kind+' ctx']:.1f}%")
d = [x[0] for x in cross]; print(f"\nacross a turn boundary ({len(cross):,} pairs): median growth {statistics.median(d):.0f}, median output of the last call {statistics.median(x[1] for x in cross):.0f}, median visible {statistics.median(x[2] for x in cross):.0f}, median added {statistics.median(x[3] for x in cross):.0f}")
print(f"  sums: growth {sum(d)/1e6:.1f}M, output {sum(x[1] for x in cross)/1e6:.1f}M, visible {sum(x[2] for x in cross)/1e6:.1f}M, added {sum(x[3] for x in cross)/1e6:.1f}M; negative growth in {sum(1 for x in d if x < -500):,} pairs")
print("\nsubagent cold wakes by pause:", dict(gaps), {k[5:]: round(v/1e6,1) for k, v in S.items() if k.startswith("gapu")})
print("what the subagent did before the pause (count, M units):")
for k, n in wake.most_common(12): print(f"  {n:5} {wakeu[k]/1e6:7.1f}M  {k}")
print("\nsubagent types (transcripts, share of subagent context):")
tot = sum(agent_units.values())
for k, n in agent_types.most_common(12): print(f"  {n:5} {100*agent_units[k]/tot:5.1f}%  {k}")
print("\nhook_success attachments (count, MB):")
for k, v in sorted(hook.items(), key=lambda kv: -kv[1])[:8]: print(f"  {hookn[k]:7,} {v/1e6:8.1f}MB  {k}  {sample[k]}"[:420])
