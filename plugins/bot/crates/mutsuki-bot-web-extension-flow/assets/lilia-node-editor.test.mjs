import assert from "node:assert/strict";
import { clientToWorld, edgePath, NODE_HEADER, NODE_WIDTH, portAnchor, PORT_ROW, samePortType } from "./lilia-node-editor.js";
import { collectLeaves, matchLeaf } from "./lilia-context-menu.js";

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

// Context menu leaf collection mirrors LiliaUI ContextMenuHost: headers are
// skipped, parent groups become the leaf's groupLabel, leaves stay ordered.
const menu = [
  {
    label: "来源",
    children: [
      { header: true, label: "QQ" },
      { id: "mutsuki.bot.qq.message.created", label: "消息创建", keywords: ["mutsuki.bot.qq.message.created", "QQ", "来源"] },
      { id: "mutsuki.bot.qq.send", label: "发送消息", keywords: ["mutsuki.bot.qq.send", "QQ", "来源"] },
    ],
  },
  {
    label: "匹配",
    children: [
      { header: true, label: "匹配" },
      { id: "mutsuki.bot.match.conversation", label: "会话匹配", keywords: ["mutsuki.bot.match.conversation", "匹配"] },
    ],
  },
];

const leaves = collectLeaves(menu);
assert.equal(leaves.length, 3);
assert.equal(leaves[0].item.id, "mutsuki.bot.qq.message.created");
assert.equal(leaves[0].groupLabel, "来源");
assert.equal(leaves[1].groupLabel, "来源");
assert.equal(leaves[2].item.id, "mutsuki.bot.match.conversation");
assert.equal(leaves[2].groupLabel, "匹配");

assert.equal(matchLeaf(leaves[0], "消息创建"), true);
assert.equal(matchLeaf(leaves[0], "mutsuki.bot.qq.message.created"), true);
assert.equal(matchLeaf(leaves[0], "qq"), true);
assert.equal(matchLeaf(leaves[0], "来源"), true);
assert.equal(matchLeaf(leaves[2], "会话"), true);
assert.equal(matchLeaf(leaves[2], "不存在"), false);
