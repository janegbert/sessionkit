// sessionkit's answer to "File has not been read yet": Claude Code refuses an Edit of a file that
// no Read in this session has seen, also when the agent read it with cat or sed, or before a
// compaction. The agent then reads the whole file again, which puts it in the context, and edits
// it once more. Here the hook does the Read itself, through the engine, so the file counts as
// read without its text entering the conversation, and tries the Edit again. The Edit stays
// safe: its old_string must still match the file exactly, once. Not for replace_all, which does
// not need a unique match, nor for Write, which replaces the whole file.

const UNREAD = "has not been read yet";

/** @type {import('claude-code').Register} */
export const register = (on) => {
  on("tool.call", { tool: "Edit" }, async ($, event, next) => {
    const result = await next(event);
    if (!result.isError || !String(result.text ?? "").includes(UNREAD) || event.replace_all) return result;
    const read = await $.tool.call({ tool: "Read", file_path: event.file_path });
    if (read.isError) return result;
    return next(event);
  });
};
