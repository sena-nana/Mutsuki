// The database browser renders arbitrary SQLite cell text, including sandbox message
// bodies that originate from QQ users and JSON columns full of quotes. Every value is
// interpolated into a template literal -- some of them inside attributes -- so the
// escaper is the only thing standing between stored bytes and executable markup.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const source = readFileSync(fileURLToPath(new URL("./index.js", import.meta.url)), "utf8");

// The module mounts a live extension, so lift the helper rather than importing it.
const body = source.slice(source.indexOf("function escapeHtml"));
const escapeHtml = new Function(`${body.slice(0, body.indexOf("\n}") + 2)}; return escapeHtml;`)();

assert.equal(escapeHtml("<script>"), "&lt;script&gt;");
assert.equal(escapeHtml("a & b"), "a &amp; b");

// A double quote must not be able to close an attribute: `title="${escapeHtml(cell)}"`
// would otherwise let a stored cell append its own event handler.
assert.equal(
  escapeHtml('" onmouseover="alert(1)'),
  "&quot; onmouseover=&quot;alert(1)",
);
assert.ok(!escapeHtml('" onmouseover="x').includes('"'));
assert.ok(!escapeHtml("' onmouseover='x").includes("'"));

// Ampersand must be escaped first, or the other replacements get double-encoded.
assert.equal(escapeHtml("&lt;"), "&amp;lt;");

// JSON cells are the common case, not an edge case.
assert.equal(
  escapeHtml('{"reply_id":"r-1"}'),
  "{&quot;reply_id&quot;:&quot;r-1&quot;}",
);

console.log("database escapeHtml ok");
