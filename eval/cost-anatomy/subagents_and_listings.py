import json, os, re, statistics
from collections import Counter, defaultdict
PROJECTS = os.path.expanduser("~/.claude/projects")
def files():
    out = []
    for folder in sorted(os.listdir(PROJECTS)):
        path = os.path.join(PROJECTS, folder)
        if not os.path.isdir(path): continue
        for name in sorted(os.listdir(path)):
            if name.endswith(".jsonl"):
                out.append((os.path.join(path, name), "main", folder))
                sub = os.path.join(path, name[:-6], "subagents")
                if os.path.isdir(sub):
                    out.extend((os.path.join(sub, f), "sub", folder) for f in sorted(os.listdir(sub)) if f.endswith(".jsonl"))
    return sorted(out, key=lambda x: x[1])
ERRAND = re.compile(r"\b(git (commit|push|pull|fetch|merge|rebase|tag|switch|checkout|add|stash|worktree)|gh (pr|release|run|api|issue|workflow)|brew|npm publish|cargo publish)\b")
TEST = re.compile(r"\b(cargo (test|build|check|clippy)|npm (run|test)|pnpm|yarn|pytest|phpunit|php artisan test|pest|phpstan|vitest|tsc|sail (test|artisan|pest)|make|docker compose|docker)\b")
LOOK = re.compile(r"^\s*(cd\s+\S+\s*(&&|;)\s*)?(cat|sed -n|grep|rg|ls|find|head|tail|wc|git (log|diff|status|show|branch)|tree)\b")
S = defaultdict(float); N = Counter(); peaks = {"main": [], "sub": []}
sim = defaultdict(float); post = []
listed = defaultdict(lambda: [0, 0.0]); called = Counter(); skills_listed = [0, 0.0]; skill_calls = Counter(); sessions_with = defaultdict(set); sessions_call = defaultdict(set)
seen = set()
for path, kind, folder in files():
    calls = []; ids = {}; after = False; compactions = 0
    try: handle = open(path, "rb")
    except OSError: continue
    for raw in handle:
        try: e = json.loads(raw)
        except ValueError: continue
        if e.get("subtype") == "compact_boundary":
            compactions += 1; after = True; continue
        if e.get("type") == "attachment":
            a = e.get("attachment") or {}
            if a.get("type") in ("deferred_tools_delta", "deferred_tools_record"):
                dump = json.dumps(a)
                for server in re.findall(r"mcp__(.+?)__[A-Za-z0-9_-]+", dump):
                    listed[server][1] += 1
                for server in set(re.findall(r"mcp__(.+?)__", dump)):
                    sessions_with[server].add(path)
                S["deferred chars"] += len(dump); N["deferred attachments"] += 1
            elif a.get("type") == "skill_listing":
                skills_listed[0] += 1; skills_listed[1] += len(json.dumps(a))
            continue
        m = e.get("message")
        if not isinstance(m, dict) or e.get("type") != "assistant": continue
        u = m.get("usage")
        if not u or m.get("model") == "<synthetic>": continue
        cid = m.get("id") or e.get("requestId")
        if cid not in ids:
            if cid in seen: ids[cid] = None; continue
            seen.add(cid)
            read = u.get("cache_read_input_tokens") or 0; write = u.get("cache_creation_input_tokens") or 0
            call = {"ctx": (u.get("input_tokens") or 0) + read + write, "read": read, "write": write, "tools": [], "after": after}
            after = False; ids[cid] = call; calls.append(call)
        call = ids[cid]
        if call and isinstance(m.get("content"), list):
            for b in m["content"]:
                if isinstance(b, dict) and b.get("type") == "tool_use":
                    name = b.get("name", ""); call["tools"].append((name, b.get("input") or {}))
                    if name.startswith("mcp__"):
                        server = name.split("__")[1]; called[server] += 1; sessions_call[server].add(path)
                    if name == "Skill": skill_calls[(b.get("input") or {}).get("skill", "?")] += 1
    handle.close()
    if not calls: continue
    N[kind + " transcripts"] += 1; N[kind + " compactions"] += compactions
    peak = max(c["ctx"] for c in calls); peaks[kind].append(peak)
    for c in calls:
        if c["after"] and c["ctx"] > 20_000: post.append((kind, c["read"] / c["ctx"], c["ctx"]))
    # compact at T, keep a share, rewrite what is kept
    base = sum(0.1 * c["ctx"] for c in calls); sim[kind + " base"] += base
    for T in (100_000, 150_000, 200_000, 342_000):
        for keep in (0.16, 0.30):
            offset = 0.0; cost = 0.0; n = 0
            for c in calls:
                if c["after"]: offset = 0.0
                size = max(c["ctx"] - offset, 20_000)
                if size > T:
                    offset += size * (1 - keep); size *= keep; cost += 2 * size if kind == "main" else 1.25 * size; n += 1
                cost += 0.1 * size
            sim[f"{kind} T={T//1000}K keep={keep}"] += cost; sim[f"{kind} T={T//1000}K keep={keep} n"] += n
    if kind == "main":
        for c in calls:
            if c["ctx"] < 150_000: continue
            S["main ctx above 150K"] += c["ctx"]
            if len(c["tools"]) == 1 and c["tools"][0][0] == "Bash":
                cmd = c["tools"][0][1].get("command", "")
                group = "errand (git, gh, release)" if ERRAND.search(cmd) else "test, build, run" if TEST.search(cmd) else "look (cat, sed, grep, ls, git log/diff)" if LOOK.match(cmd) else "other Bash"
                S["main>150K " + group] += c["ctx"]; N["main>150K " + group] += 1
for kind in ("main", "sub"):
    p = sorted(peaks[kind]); q = lambda f: p[int(f * (len(p) - 1))] / 1000
    print(f"{kind}: {N[kind+' transcripts']:,} transcripts, {N[kind+' compactions']:,} compactions; peak context median {q(.5):.0f}K p75 {q(.75):.0f}K p90 {q(.9):.0f}K max {q(1):.0f}K; above 200K: {sum(1 for x in p if x > 200_000):,}, above 400K: {sum(1 for x in p if x > 400_000):,}")
    for T in (100, 150, 200, 342):
        print("   " + "; ".join(f"compact at {T}K keeping {int(keep*100)}%: reads+rewrites {100*sim[f'{kind} T={T}K keep={keep}']/sim[kind+' base']-100:+.0f}% ({sim[f'{kind} T={T}K keep={keep} n']:,.0f} compactions)" for keep in (0.16, 0.30)))
print(f"\nfirst call after a compaction ({len(post)}): median share read from cache {statistics.median(x[1] for x in post):.2f}, median context {statistics.median(x[2] for x in post)/1000:.0f}K; share with more than half read from cache: {sum(1 for x in post if x[1] > .5)/len(post):.2f}")
print("\nmain calls above 150K with one Bash (share of main context above 150K):")
for k in sorted((k for k in S if k.startswith("main>150K")), key=lambda k: -S[k]): print(f"  {N[k]:6,}  {100*S[k]/S['main ctx above 150K']:5.1f}%  {k[10:]}")
print(f"\ndeferred tool listings: {N['deferred attachments']:,} attachments, {S['deferred chars']/1e6:.1f}MB")
for server, (_, names) in sorted(listed.items(), key=lambda kv: -kv[1][1])[:14]:
    print(f"  {names:9,.0f} names listed in {len(sessions_with[server]):4} transcripts; called {called[server]:5} times in {len(sessions_call[server]):3} transcripts  {server[:40]}")
print(f"skill listings: {skills_listed[0]:,} attachments, {skills_listed[1]/1e6:.1f}MB; Skill calls: {sum(skill_calls.values())} — {skill_calls.most_common(8)}")
