// sessionkit against a report that arrives twice: a teammate sends its report with SendMessage,
// and when it goes idle its notification carries the same text again as its result. The lead
// then reads the report a second time and spends a turn saying it already had it. In the
// transcripts here, 101 of 616 idle notifications with a result repeated a message from the same
// teammate, and 75 were followed by a turn without tools, at a median context of 371K tokens.
// The notification still arrives, so a lead that waits for it still wakes; only its result is
// replaced by a line that names the earlier message.

const MIN_CHARS = 200; // a short result costs little either way
const SIMILAR = 0.6; // shared words, against the shorter text: the line the measurement used
const KEPT = 5; // earlier messages per teammate to compare with

const recent = new Map(); // loop and teammate → [{ text, at }]

const words = (text) => new Set(text.toLowerCase().match(/[\p{L}\p{N}_]{4,}/gu) ?? []);

function similarity(a, b) {
  const [x, y] = [words(a), words(b)];
  if (x.size === 0 || y.size === 0) return 0;
  let shared = 0;
  for (const word of x) if (y.has(word)) shared += 1;
  return shared / Math.min(x.size, y.size);
}

/** The idle notification in a delivery's text, with where it sits, or null. */
function idleNotification(text) {
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start < 0 || end < start) return null;
  try {
    const value = JSON.parse(text.slice(start, end + 1));
    return value?.type === "idle_notification" ? { value, start, end } : null;
  } catch {
    return null;
  }
}

const clock = (ms) => new Date(ms).toISOString().slice(11, 16);

/** @type {import('claude-code').Register} */
export const register = (on) => {
  on("session.receive", async ($, event, next) => {
    const teammate = event.origin?.teammate;
    if (!teammate) return next(event);
    const key = `${event.agentId ?? "main"}:${teammate}`;
    const earlier = recent.get(key) ?? [];
    const idle = idleNotification(event.text);
    if (!idle) {
      recent.set(key, [...earlier, { text: event.text, at: Date.now() }].slice(-KEPT));
      return next(event);
    }
    const result = typeof idle.value.result === "string" ? idle.value.result : "";
    const repeated = result.length >= MIN_CHARS && earlier.find((m) => similarity(result, m.text) >= SIMILAR);
    if (!repeated) return next(event);
    const note = `sessionkit: left out, it repeats the message ${teammate} sent at ${clock(repeated.at)} UTC.`;
    const text = event.text.slice(0, idle.start) + JSON.stringify({ ...idle.value, result: note }) + event.text.slice(idle.end + 1);
    return next({ ...event, text });
  });
};
