// sessionkit's tip after a large read: agents read a whole file for one fact out of habit, and
// the text stays in the context for every later turn. After the first such read in a loop (the
// main one, or a subagent's), one line tells the agent that sessionkit ask answers a question
// about a file without reading it. The read itself goes ahead unchanged.
//
// With the option readOutline, the first whole read of such a file in a loop shows its first
// lines and, below them, every declaration and heading with its line number, from `sessionkit
// trim --declarations`; the agent reads the parts it needs with offset and limit. A second whole
// read of the same file gets all of it. The engine has read the whole file either way, so an
// Edit of it is allowed.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 5_000;
const MIN_LINES = 400; // below this a read is cheap enough
const CHARS_PER_TOKEN = 4;

const hinted = new Set(); // loops that had the tip: once is enough to teach it
const outlined = new Set(); // loop and file: a second whole read of a file gets all of it

/** The first lines of the file and the note with its declarations, or null when sessionkit fails. */
async function outline($, file, path) {
  try {
    const run = await $.process.run([SESSIONKIT, "trim", "--declarations"], { stdin: file.content, timeoutMs: TIMEOUT_MS });
    const { head, declarations } = JSON.parse(run.stdout);
    const note =
      `sessionkit: ${path} has ${file.numLines} lines. To keep the context small, this read shows lines 1-${head}, and below ` +
      `every declaration and heading with its line number. Read the parts you need with offset and limit. To get the whole ` +
      `file, read it again: a second whole read of it is not shortened.\n${declarations.join("\n")}`;
    return { head: file.content.split("\n").slice(0, head).join("\n"), lines: head, note };
  } catch {
    return null;
  }
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  on("tool.call", { tool: "Read" }, async ($, event, next) => {
    const result = await next(event);
    if ("deny" in result || result.isError || result.result?.type !== "text") return result;
    const file = result.result.file;
    if (event.offset !== undefined || event.limit !== undefined || file.startLine > 1 || file.numLines < MIN_LINES) return result;
    const loop = `${await $.session.id()}:${event.agentId ?? "main"}`;
    const key = `${loop}:${file.filePath}`;
    if (options?.readOutline && !file.truncatedByTokenCap && !outlined.has(key)) {
      outlined.add(key);
      const cwd = await $.session.cwd();
      const shown = await outline($, file, file.filePath.startsWith(cwd + "/") ? file.filePath.slice(cwd.length + 1) : file.filePath);
      if (shown) {
        return { result: { ...result.result, file: { ...file, content: shown.head, numLines: shown.lines } }, context: [...(result.context ?? []), shown.note] };
      }
    }
    if (hinted.has(loop)) return result;
    hinted.add(loop);
    const cwd = await $.session.cwd();
    const path = file.filePath.startsWith(cwd + "/") ? file.filePath.slice(cwd.length + 1) : file.filePath;
    const tokens = Math.round(file.content.length / CHARS_PER_TOKEN / 1000);
    const tip =
      `sessionkit: this read put ${file.numLines} lines (about ${tokens}k tokens) in the context, where they stay for every later turn. ` +
      `When you need a fact about a large file rather than its text, ask instead: sessionkit ask ${path} "Does it retry a failed upload?" ` +
      `answers yes or no, and sessionkit ask --find ${path} "Where is the timeout set?" names the line, so you can Read only that part ` +
      `with offset and limit. To find where a behavior lives in the repository: sessionkit grep "question". ` +
      `Read the whole file when you are going to edit much of it.`;
    return { ...result, context: [...(result.context ?? []), tip] };
  });
};
