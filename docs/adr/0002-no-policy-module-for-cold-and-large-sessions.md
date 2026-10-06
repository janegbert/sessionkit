# No policy module for cold and large sessions

The settings hook (`prompt` in `src/claude.rs`) and the plugin hooks (`cold_check`, `cold_choice`) each decide what to do with a message to a cold or large session. We do not extract a shared, pure policy module. The decision itself is a few comparisons; the work is side effects: ack files, the Claude Code pid, the handoff and a Jev question. A pure module would take about six booleans and return one of three answers, which hides nothing.

The two paths also find Claude Code's pid differently on purpose: the settings hook reads `CLAUDE_PID`; a function hook is started without a shell, so `claude_pid` falls back to the parent process.
