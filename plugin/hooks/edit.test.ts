// The retry of an Edit refused for an unread file, against the engine; Read and Edit are stood in
// for beneath the plugin.
import { expect, test } from "claude-code/testing";

const REFUSED = { result: "File has not been read yet. Read it first before writing to it.", isError: true, text: "File has not been read yet. Read it first before writing to it." };

function world(on) {
  const seen = { reads: [], edits: 0 };
  let read = false;
  on("tool.call", { tool: "Read" }, (_$, e) => {
    seen.reads.push(e.file_path);
    read = true;
    return { result: { type: "text", file: { filePath: e.file_path, content: "a\n", numLines: 1, startLine: 1, totalLines: 1 } } };
  });
  on("tool.call", { tool: "Edit" }, () => {
    seen.edits += 1;
    return read ? { result: { filePath: "/repo/a.rs" }, text: "The file /repo/a.rs has been updated." } : REFUSED;
  });
  return seen;
}

const EDIT = { tool: "Edit", file_path: "/repo/a.rs", old_string: "let a = 1;", new_string: "let a = 2;" };

test("a refused edit of an unread file is read and tried again", async ($, on) => {
  const seen = world(on);
  const result = await $.tool.call(EDIT);
  expect(result.isError).toBeUndefined();
  expect(seen.reads).toEqual(["/repo/a.rs"]);
  expect(seen.edits).toBe(2);
});

test("replace_all is not tried again", async ($, on) => {
  const seen = world(on);
  const result = await $.tool.call({ ...EDIT, replace_all: true });
  expect(result.isError).toBe(true);
  expect(seen.reads).toEqual([]);
});

test("another error goes through as it is", async ($, on) => {
  const seen = { reads: 0 };
  on("tool.call", { tool: "Read" }, () => { seen.reads += 1; return { result: "x" }; });
  on("tool.call", { tool: "Edit" }, () => ({ result: "String to replace not found in file.", isError: true, text: "String to replace not found in file." }));
  const result = await $.tool.call(EDIT);
  expect(result.text).toContain("not found");
  expect(seen.reads).toBe(0);
});
