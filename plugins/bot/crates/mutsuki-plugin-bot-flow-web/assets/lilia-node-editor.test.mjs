import assert from "node:assert/strict";
import { edgePath, portAnchor, samePortType } from "./lilia-node-editor.js";

assert.equal(samePortType(
  { type_id: "mutsuki.bot.event", version: 1 },
  { type_id: "mutsuki.bot.event", version: 1 },
), true);
assert.equal(samePortType(
  { type_id: "mutsuki.bot.event", version: 1 },
  { type_id: "mutsuki.bot.flow.error", version: 1 },
), false);
assert.equal(samePortType(null, { type_id: "mutsuki.bot.event", version: 1 }), false);

const start = portAnchor({ x: 40, y: 80 }, { direction: "output" }, 0);
const end = portAnchor({ x: 320, y: 160 }, { direction: "input" }, 1);
assert.equal(start.x, 260);
assert.equal(start.y, 146);
assert.equal(end.x, 320);
assert.equal(end.y, 254);

const path = edgePath(start.x, start.y, end.x, end.y);
assert.match(path, /^M 260 146 C /);
assert.match(path, /, 320 254$/);
