import assert from "node:assert/strict";
import { clientToWorld, edgePath, NODE_HEADER, NODE_WIDTH, portAnchor, PORT_ROW, samePortType } from "./lilia-node-editor.js";

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
assert.equal(start.x, 40 + NODE_WIDTH);
assert.equal(start.y, 80 + NODE_HEADER);
assert.equal(end.x, 320);
assert.equal(end.y, 160 + NODE_HEADER + PORT_ROW);

const path = edgePath(start.x, start.y, end.x, end.y);
assert.match(path, /^M 260 151 C /);
assert.match(path, /, 320 259$/);

const view = { scale: 1.25 };
const worldRect = { left: 100, top: 80 };
const got = clientToWorld(worldRect, view, worldRect.left + 40 * view.scale, worldRect.top + 16 * view.scale);
assert.equal(got.x, 40);
assert.equal(got.y, 16);
