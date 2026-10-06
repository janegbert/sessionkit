// sessionkit's function hooks: a plugin names one module, so this one registers them all.
import { register as agent } from "./agent.js";
import { register as architect } from "./architect.js";
import { register as awareness } from "./awareness.js";
import { register as compact } from "./compact.js";
import { register as edit } from "./edit.js";
import { register as effort } from "./effort.js";
import { register as fold } from "./fold.js";
import { register as prompt } from "./prompt.js";
import { register as permissionProbe } from "./permission-probe.js";
import { register as autoPermission } from "./auto-permission.js";
import { register as read } from "./read.js";
import { register as repeat } from "./repeat.js";
import { register as searchTools } from "./search-tools.js";

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  agent(on, options);
  architect(on, options);
  awareness(on, options);
  compact(on, options);
  edit(on, options);
  effort(on, options);
  fold(on, options);
  prompt(on, options);
  permissionProbe(on, options);
  autoPermission(on, options);
  read(on, options);
  repeat(on, options);
  searchTools(on, options);
};
