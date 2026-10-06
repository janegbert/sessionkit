// First-class tools over the existing CLI. Never turn a Read/Grep into a paid
// request automatically; the agent chooses the tool, whose description says
// source is sent to TypeSafe. setup replaces the executable placeholder.
const SESSIONKIT = "__SESSIONKIT__";
const SEARCH = "mcp__sessionkit__search_code";
const ASK = "mcp__sessionkit__ask_file";
const TIMEOUT_MS = 120_000;
const SECTION = "sessionkit-code-search";
export const GUIDANCE =
  "Code navigation (when these tools are available): unknown location of a behavior -> use mcp__sessionkit__search_code first; " +
  "exact symbol, path or literal -> use Grep; known file, factual questions or relevant lines -> " +
  "use mcp__sessionkit__ask_file (batch questions). These tools send eligible source to TypeSafe. " +
  "Read the returned excerpts before searching again; do not open every listed file. " +
  "Read missing context and the relevant ranges before editing. On errors or incomplete discovery, " +
  "use ordinary search/Read; a missing result is not proof that code is absent.";

const specs = [
  {
    name: "search_code",
    description: "Find implementation, callers and tests for a behavior when you do not know where it lives. " +
      "Prefer this to speculative keyword searches or reading files one by one. Returns ranked files and verbatim " +
      "source excerpts with line numbers via sessionkit grep, not a generated answer. For exact symbols/literals use Grep. " +
      "Read excerpts first, then only missing ranges. Eligible source is sent to TypeSafe (paid Jev requests, cached). " +
      "Hidden, ignored, dependency and sensitive files stay excluded. Incomplete discovery is not evidence of absence.",
    inputSchema: { type: "object", additionalProperties: false, required: ["question"], properties: {
      question: { type: "string", minLength: 1, maxLength: 4096, description: "One natural-language behavior to locate." },
      root: { type: "string", minLength: 1, description: "Search folder; default is the session working directory. Use an explicit root in a subagent with a different cwd." },
      max_source_bytes: { type: "integer", minimum: 1, maximum: 100000, default: 12000, description: "Budget for returned source excerpts; not a completeness guarantee." },
    } },
  },
  {
    name: "ask_file",
    description: "Ask factual questions about a known file without putting its whole contents in context. " +
      "Batch up to ten questions in one call. mode=find locates relevant lines for one question. " +
      "Uses sessionkit ask; returns probabilities and supporting lines, not guaranteed facts. " +
      "Read around evidence before editing; use Read for broad edits or uncertain answers. " +
      "Source is sent to TypeSafe (paid Jev requests); sensitive names and private-key content stay excluded.",
    inputSchema: { type: "object", additionalProperties: false, required: ["path", "questions"], properties: {
      path: { type: "string", minLength: 1, description: "File path; relative to the session working directory. Use absolute paths in a subagent with a different cwd." },
      questions: { type: "array", minItems: 1, maxItems: 10, items: { type: "string", minLength: 1, maxLength: 4096 } },
      mode: { type: "string", enum: ["facts", "find"], default: "facts", description: "find requires exactly one question." },
    } },
  },
];
const text = (value) => typeof value === "string" && value.trim().length > 0 && value.length <= 4096;

async function run($, argv) {
  try {
    const result = await $.process.run(argv, { timeoutMs: TIMEOUT_MS });
    const status = result.exitCode === 0 ? "complete" : result.exitCode === 2 && argv[1] === "grep" ? "incomplete" : "failed";
    const cut = result.isStdoutTruncated || result.isStderrTruncated;
    const output = `sessionkit: ${status} (exit ${result.exitCode})${cut ? "; process output truncated" : ""}.\n` +
      result.stdout + (result.stderr ? `\nDiagnostics:\n${result.stderr}` : "");
    return { result: output + (status !== "complete" || cut ?
      "\nMissing evidence is unknown. Use Grep/Read to verify gaps; do not treat this as a negative finding." :
      "\nUse the excerpts first; Read only missing context and ranges needed for an edit.") };
  } catch {
    return { result: "sessionkit: could not run the CLI (missing executable, timeout, or process failure). " +
      "Use Grep/Read instead; no conclusion can be drawn from this failure." };
  }
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (options?.codeSearchTools === false) return;
  let ready = false;
  on("session.start", async ($, event, next) => {
    try {
      for (const spec of specs) await $.tool.register(spec);
      ready = true;
    } catch {
      ready = false;
      $.ui.log("sessionkit: code search tools could not be registered; use the CLI or ordinary search.");
    }
    return next(event);
  });
  // Keep just these two small schemas visible instead of hiding them behind
  // ToolSearch, where an agent already needs to know what to ask for.
  on("tool.describe", { tool: /^mcp__sessionkit__(search_code|ask_file)$/ }, async ($, event, next) => ({
    ...await next(event), isDeferred: false,
  }));
  on("prompt.compose", async ($, event, next) => {
    const result = await next(event);
    if (!ready || !event.tools.includes(SEARCH) || !event.tools.includes(ASK) ||
        result.sections.some((section) => section.id === SECTION)) return result;
    return { ...result, sections: [...result.sections, { id: SECTION, scope: "session", text: GUIDANCE }] };
  });
  // Spawn inputs do not expose the child's tool allowlist. Make the instruction
  // conditional; prompt.compose additionally checks the actual offered tools.
  on("agent.spawn", { prompt: /./s }, async ($, event, next) => {
    if (!ready) return next(event);
    if (event.prompt.includes(GUIDANCE)) return next(event);
    return next({ ...event, prompt: `${event.prompt}\n\n${GUIDANCE}` });
  });
  on("tool.call", { tool: SEARCH }, async ($, event) => {
    const budget = event.max_source_bytes ?? 12000;
    if (!text(event.question) || (event.root !== undefined && !text(event.root)) ||
        !Number.isInteger(budget) || budget < 1 || budget > 100000) {
      return { result: "sessionkit: invalid search input. Supply a question, optional root and a source budget of 1..100000 bytes." };
    }
    // -- ends CLI option parsing, so question/root can never enable unsafe flags.
    return run($, [SESSIONKIT, "grep", "--max-source-bytes", String(budget), "--", event.question, event.root ?? "."]);
  });
  on("tool.call", { tool: ASK }, async ($, event) => {
    const mode = event.mode ?? "facts";
    if (!text(event.path) || !Array.isArray(event.questions) || event.questions.length < 1 || event.questions.length > 10 ||
        event.questions.some((q) => !text(q) || q.startsWith("-")) ||
        !["facts", "find"].includes(mode) || (mode === "find" && event.questions.length !== 1)) {
      return { result: "sessionkit: invalid file question input. Give a file, 1..10 plain questions (not CLI flags), " +
        "and facts or find mode. find requires one question." };
    }
    // ask has no -- delimiter. An absolute file path and validated questions
    // prevent arguments being interpreted as --include-sensitive or other flags.
    const path = event.path.startsWith("/") ? event.path : `${await $.session.cwd()}/${event.path}`;
    return run($, [SESSIONKIT, "ask", ...(mode === "find" ? ["--find"] : []), path, ...event.questions]);
  });
};
