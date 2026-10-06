# Cost anatomy, 2026-10-02

Where the cost of Claude Code goes on the maintainer's machine, by the choice that caused it, and
ten candidate savings that sessionkit does not have yet. Measured over all transcripts under
`~/.claude/projects`: 1,668 transcripts, 79,215 API calls, $11,773 API-equivalent.

Everything here is local and read-only. Nothing is built yet; each candidate names what is not
verified.

## How to run

```
python3 eval/cost-anatomy/anatomy.py                 # cost by choice: prefix, calls, prompts, errors, pauses
python3 eval/cost-anatomy/context_parts.py           # what the context that is read again consists of
python3 eval/cost-anatomy/output_and_wakes.py        # output counting, thinking, cold wakes of subagents
python3 eval/cost-anatomy/subagents_and_listings.py  # subagent compaction simulated, unused listings
python3 eval/cost-anatomy/rules.py                   # deterministic rules on tool results
```

Each takes 10 to 30 seconds. Cost is in input-token units: uncached input 1, cache read 0.1,
cache write 1.25 (5 minutes) or 2 (1 hour), output 5. The units mix models, so a share is an
estimate, not a bill. A token is 4 characters where the transcript gives only text.

## What the money is

| | share of cost |
|---|---|
| cache reads | 70% |
| cache writes | 22% |
| output | 8% |
| main sessions | 49% |
| subagents | 51% |

What a main session reads again, as a share of its context read: assistant output 35% (of which
visible text and tool inputs 14%; the rest is thinking), the fixed prefix 18%, tool results 17%,
listings and reminders about 12%. For a subagent: tool results 26%, prefix 22%, output 15%.
The prefix and the listings overlap, so the parts add up to a little under 100% by accident.

## Ten candidates, by measured size

Sizes are ceilings or simulations. The quality of the work after a compaction or a fold is not
modelled.

1. **Compact subagents.** Subagents are 51% of cost; 96% of that sits in subagents of 31 calls
   or more. Their context peaks at a median 181K, 266 of 614 pass 200K, and they compacted twice
   in total. sessionkit's dialogs reach the person in the main session only. Simulated, a
   compaction at 150K removes 56% to 61% of subagent reads plus rewrites, about 20% of all cost.
   Verified 2026-10-02: the compact hook fires inside a subagent (one went from 341K to 126K
   without a summary). A subagent takes the main session's auto-compact window and compacts at the
   window minus 33K; Claude Code 2.1.287 has no window per agent, and with no window set a
   subagent never compacts. At the inherited 375K window the simulation gives 21% to 25%.
2. **Fold a turn at its boundary.** A cut in the middle of the context rewrites the cache; a cut
   at the tail does not. Right after a turn, its tool results, thinking and tool inputs can go and
   its prompt, final text and changed files stay. Tool results are read again 80% in later turns,
   and only 4% of the Read volume read again later belongs to a file a later turn touched.
   Ceiling about 14% of all cost, main sessions only. Jev asks the existing compact question about
   the calls of that one turn. Verified 2026-10-02 with Claude Code 2.1.287 and Haiku: a plugin
   can start a compaction at `turn.complete` that its own hook answers, and when only the user
   messages with tool results of the last turn are rebuilt, the next call read 51.6K from the
   cache and wrote 0.8K (context before: 74.5K). A rebuilt assistant message loses the cache
   (read 18.0K, wrote 43.5K). A regular sessionkit compaction rebuilds assistant messages too, but
   keeping them would gain nothing there: in 7 of 8 compactions of real sessions the first changed
   message is among the first 2 to 5, with 0 to 2% of the kept conversation before it (one case:
   35%), so the rest is written again whatever is rebuilt (`first_cut.py`). After those
   compactions the first call read 26–38K, the system prompt and tools, and wrote 52–130K. Built
   as the option *Fold a turn when it ends*, without Jev, off by default.
3. **Effort per prompt.** About two thirds of the output of main sessions is thinking (18M of
   26.8M tokens). It is paid as output and then stays in the context across turns: together up to
   11% of all cost. An upper bound, because the visible part is estimated at 4 characters a token.
   Jev picks the effort per prompt, in shadow first, as it does for subagents; the status line
   shows the share of thinking. Verified 2026-10-02: a `turn.step` hook can rewrite the effort.
   Anthropic documents that a change of the request's effort invalidates the cached messages
   unless it is carried inside the conversation, and Claude Code sends a hook's change that way:
   on Opus 5.5 the lowered turn read 121.4K from the cache and wrote 0.2K, and so did the turn
   after it (Sonnet 5.5 the same at 90K). Built in shadow, with applying as an option that is off
   until Jev's answers are measured.
4. **A diet for listings.** The fixed start of a session is 18% of main and 22% of subagent
   context read (first call: median 36K and 48K). One MCP server put 419,698 tool names in 377
   transcripts and was never called: about 1.9% of all cost. Skill listings are 25.6MB for 153
   Skill calls, about 1.6%. A `sessionkit diet` shows per project what is installed, what was
   used and what it costs, and writes the switch-off per project; subagents get a lean definition.
5. **Fewer, fatter calls.** A call reads the whole context and carries 1.07 tools on average.
   4,595 main calls and 4,844 subagent calls only looked, right after another look, and asked for
   nothing the previous result named: about 5% of all cost. A hook names the price of the split
   at that moment; the general cost note has not changed the habit.
6. **Hand errands to a lean subagent.** Above 150K, 6.4% of main context read goes to git and gh
   errands and 13.4% to test, build and run. A fresh small context does these at about a sixth
   per call and keeps the test output out of the main context. Jev judges whether a prompt or
   command is an errand that needs none of the conversation. Ceiling about 5%.
7. **Wait with the right cache lifetime.** Subagents write a 5-minute cache, main sessions a
   1-hour one. 413 times a subagent woke after 5 to 60 minutes and rewrote its whole context: 5.2%
   of all cost; 165 after a foreground Bash, 114 while waiting for a message. Claude Code has
   `subagentPromptCacheTtl`, `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL` and a TTL in an agent's
   frontmatter. One hour for every subagent nets about 1%, because a write then costs 2 instead of
   1.25; the gain is in agents that wait, or in a poll that keeps the cache warm.
8. **A ledger of refusals.** 2,817 failed calls carry 3.3% of context read, and they repeat word
   for word: 625 "isolated in the worktree", 162 denied by the auto-mode classifier, 91 "File does
   not exist". As `skills` finds repeated work, this finds repeated refusals and puts the rule at
   the start of a subagent. About 1.5% to 2%.
9. **Jev searches before the agent does.** Agents choose `sessionkit grep` in 1% to 2% of their
   searches. Looking at the start of a turn is 5.0% of main context read; look-after-look is
   11.9% in subagents. The hook runs the search itself, at the prompt and when a subagent starts,
   and hands over the places. About 2% to 4%.
10. **The bill by choice.** `usage` shows kinds of token and models. A person chooses other
    things: teams and subagents (51%), effort (up to 11%), installed servers and skills (4%),
    staying above 200K (half of main context read lies above it), long foreground commands in
    subagents (2%). The price belongs at the moment of the choice.

## Measured and dropped

- Rules on tool results without a judgment: noise filters 0.5% of Bash, identical output 0.1%,
  the same command again 0.1% to 0.3%, lines already in the context 4.0% of Bash and 3.5% of
  Read. Collapsing lines that differ only in digits cut real data rows. Bash results are source
  code read through `cat`, `sed` and `grep` (62% of Bash tokens).
- Keeping a main session warm with a ping every 55 minutes: net +0.5% at best, negative beyond 4
  hours.
- Short confirmations such as "ja" (0.3%), turns that only ask back (0.1%), interrupted turns
  (0.5%), corrections (0.1%).
- Compaction that spares the cache: the first call after a compaction reads a median 34% from
  the cache, and what is left is small.

## Found on the way

`sessionkit usage` took `output_tokens` from the first transcript line of a call. Claude Code
writes a line per block, and for subagents the first line often carries the count of the
stream's start. Output was 31.8M tokens and is 48.6M; fixed in `read_calls`.
