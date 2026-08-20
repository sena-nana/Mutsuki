const STYLE = `
.console-page-content:has(.sandbox-client) {
  display: flex; flex-direction: column; overflow: hidden;
}
.console-page-content:has(.sandbox-client) > .page-body {
  display: flex; flex: 1; min-width: 0; min-height: 0;
}
.sandbox-client {
  position: relative; display: flex; flex-direction: column;
  flex: 1; min-height: 0; min-width: 0; height: auto;
}
.sandbox-frame { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 220px) minmax(0, 1fr) minmax(0, 168px); border: 1px solid var(--border, transparent); border-radius: 16px; overflow: hidden; }
.sandbox-pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; overflow: hidden; }
.sandbox-pane + .sandbox-pane { border-left: 1px solid var(--border, transparent); }
.sandbox-pane-head { display: flex; align-items: center; gap: 6px; padding: 8px 10px; border-bottom: 1px solid var(--border, transparent); flex: 0 0 auto; }
.sandbox-pane-head h2 { margin: 0; font-size: 13px; font-weight: 650; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-mode-tabs { width: 100%; height: auto; }
.sandbox-mode-tabs button { flex: 1; justify-content: center; height: 26px; font-size: 12px; }
.sandbox-status { margin: 0; padding: 0 10px 6px; font-size: 12px; }
.sandbox-status:empty, .sandbox-status[hidden] { display: none; }
.sandbox-search-row { display: flex; align-items: center; gap: 6px; padding: 8px 8px 6px; min-width: 0; flex: 0 0 auto; }
.sandbox-search-row .ui-input { flex: 1; min-width: 0; width: auto; height: 28px; margin: 0; box-sizing: border-box; }
.sandbox-client .ui-input,
.sandbox-client .ui-textarea {
  width: 100%; max-width: 100%; min-width: 0;
  border: 1px solid var(--border, transparent); border-radius: var(--radius-sm, 8px);
  background: var(--bg, transparent); color: var(--text, inherit); font: inherit;
}
.sandbox-client .ui-input { height: 30px; padding: 0 9px; font-size: 13px; }
.sandbox-client .ui-input--sm { height: 28px; padding: 0 8px; font-size: 12px; }
.sandbox-client .ui-textarea { min-height: 96px; padding: 9px; font-size: 13px; line-height: 1.45; resize: vertical; }
.sandbox-client .ui-input:focus-visible,
.sandbox-client .ui-textarea:focus-visible { border-color: var(--accent, #7aa2ff); outline: none; }
.sandbox-client .sandbox-add {
  box-sizing: border-box; width: 28px; min-width: 28px; max-width: 28px; height: 28px; padding: 0;
  display: inline-grid; place-items: center; flex: none; border-radius: 8px;
}
.sandbox-client .sandbox-add svg { width: 16px; height: 16px; display: block; }
.sandbox-draft-chip .sandbox-add { width: 18px; min-width: 18px; max-width: 18px; height: 18px; }
.sandbox-clear { margin-left: auto; flex: none; }
.sandbox-session-list, .sandbox-member-list, .sandbox-messages { flex: 1; min-height: 0; overflow: auto; }
.sandbox-client .sandbox-session, .sandbox-client .sandbox-member {
  display: grid; gap: 8px; align-items: center; justify-content: stretch; justify-items: start;
  width: 100%; height: auto; min-height: 40px; box-sizing: border-box; overflow: hidden;
  border: 0; border-radius: 0; background: transparent; text-align: left; padding: 6px 10px; cursor: pointer; color: inherit;
}
.sandbox-session { grid-template-columns: 32px minmax(0, 1fr) max-content; }
.sandbox-member { grid-template-columns: 24px minmax(0, 1fr); }
.sandbox-session > *, .sandbox-member > * { min-width: 0; }
.sandbox-session > div, .sandbox-member > div { overflow: hidden; }
.sandbox-session p, .sandbox-member p, .sandbox-bubble p { margin: 0; }
.sandbox-session-title, .sandbox-session-preview, .sandbox-member-name { max-width: 100%; }
.sandbox-session:hover, .sandbox-member:hover { background: var(--bg-hover, var(--bg-subtle, transparent)); }
.sandbox-session.is-active, .sandbox-member.is-active { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-avatar { width: 32px; height: 32px; border-radius: 50%; display: grid; place-items: center; font-size: 13px; font-weight: 650; color: var(--accent-text, #fff); background: var(--accent, #7aa2ff); flex: none; overflow: hidden; }
.sandbox-avatar--sm { width: 24px; height: 24px; font-size: 11px; }
img.sandbox-avatar { display: block; object-fit: cover; object-position: center; padding: 0; }
.sandbox-session-title, .sandbox-member-name { display: block; font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-preview { margin: 1px 0 0; font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sandbox-session-time { font-size: 10px; white-space: nowrap; min-width: max-content; justify-self: end; align-self: start; padding-top: 1px; }
.sandbox-messages { padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; background: var(--bg-subtle, transparent); }
.sandbox-row { display: flex; gap: 8px; max-width: 78%; align-items: flex-end; }
.sandbox-row--user { align-self: flex-start; }
.sandbox-row--bot { align-self: flex-end; flex-direction: row-reverse; }
.sandbox-row--system { align-self: center; max-width: 90%; }
.sandbox-row-body { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.sandbox-bubble { position: relative; border-radius: 12px; padding: 8px 12px; background: var(--bg-elev, transparent); min-width: 0; }
.sandbox-row--bot .sandbox-bubble { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-row--system .sandbox-bubble { background: transparent; }
.sandbox-client .sandbox-reply { position: absolute; top: 6px; right: 6px; opacity: 0; pointer-events: none; }
.sandbox-bubble:hover .sandbox-reply, .sandbox-bubble:focus-within .sandbox-reply { opacity: 1; pointer-events: auto; }
.sandbox-quote { margin: 0 0 6px; padding: 4px 8px; border-left: 3px solid var(--accent, #7aa2ff); opacity: 0.8; font-size: 12px; }
.sandbox-compose { display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; border-top: 1px solid var(--border, transparent); flex: 0 0 auto; }
.sandbox-compose-row { display: flex; gap: 8px; align-items: center; min-width: 0; }
.sandbox-compose-field { position: relative; flex: 1; min-width: 0; display: flex; }
.sandbox-compose-row .ui-input, .sandbox-compose-row .ui-textarea { flex: 1; min-width: 0; margin: 0; box-sizing: border-box; }
.sandbox-compose-row .ui-textarea { min-height: 64px; max-height: 160px; }
.sandbox-compose-row .sandbox-add.is-active { background: var(--accent-soft, var(--bg-hover, transparent)); }
.sandbox-pane--chat { overflow: visible; }
.sandbox-quote-bar { display: flex; align-items: center; justify-content: space-between; gap: 8px; font-size: 13px; }
.sandbox-client .sandbox-member .ghost { height: 22px; padding: 0 6px; font-size: 11px; flex: none; }
.sandbox-member-actions { display: flex; gap: 4px; flex-wrap: wrap; }
.sandbox-context-menu {
  position: absolute; z-index: 1900; min-width: 160px; padding: 4px;
  background: var(--bg-elev, var(--bg, #fff)); border: 1px solid var(--border, transparent); border-radius: 10px;
}
.sandbox-context-menu button { display: block; width: 100%; height: auto; padding: 8px 10px; border: 0; background: transparent; text-align: left; }
.sandbox-context-menu button:hover { background: var(--bg-hover, transparent); }
.sandbox-dialog-overlay { position: absolute; inset: 0; z-index: var(--z-dialog, 1800); display: grid; place-items: center; padding: 16px; box-sizing: border-box; background: color-mix(in oklch, black 42%, transparent); }
.sandbox-dialog { width: min(380px, 100%); max-height: min(520px, 100%); min-height: 0; display: flex; flex-direction: column; background: var(--lilia-surface-fill-raised, var(--bg-elev, var(--bg, #fff))); border: 1px solid var(--border, transparent); border-radius: 16px; box-shadow: var(--shadow-dialog, 0 14px 40px rgb(0 0 0 / 0.35)); overflow: hidden; }
.sandbox-dialog-head, .sandbox-dialog-foot { display: flex; align-items: center; gap: 8px; padding: 12px 14px; flex: none; }
.sandbox-dialog-head { border-bottom: 1px solid var(--border, transparent); }
.sandbox-dialog-head h2 { margin: 0; font-size: 14px; font-weight: 650; flex: 1; min-width: 0; }
.sandbox-dialog-body { padding: 12px 14px; overflow: auto; min-height: 0; display: flex; flex-direction: column; gap: 10px; }
.sandbox-dialog-field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); }
.sandbox-dialog-field .ui-input, .sandbox-dialog-field .ui-textarea { width: 100%; min-width: 0; margin: 0; box-sizing: border-box; }
.sandbox-dialog-field .ui-textarea { min-height: 72px; }
.sandbox-dialog-foot { justify-content: flex-end; border-top: 1px solid var(--border, transparent); }
.sandbox-dialog .sandbox-member { border-radius: 8px; padding: 8px 10px; }
.sandbox-dialog .sandbox-member:disabled { opacity: 0.45; cursor: default; }
.sandbox-empty { padding: 10px; margin: 0; font-size: 12px; }
.sandbox-mention { color: var(--accent, #7aa2ff); font-weight: 650; }
.sandbox-media { display: block; max-width: min(240px, 100%); max-height: 180px; border-radius: 8px; margin-top: 6px; }
.sandbox-file { display: block; margin-top: 6px; font-size: 12px; }
.sandbox-card { margin-top: 6px; min-width: 180px; max-width: 260px; border: 1px solid var(--border, transparent); border-radius: 10px; overflow: hidden; background: var(--bg, transparent); }
.sandbox-card img { display: block; width: 100%; max-height: 120px; object-fit: cover; }
.sandbox-card-body { padding: 8px 10px; display: flex; flex-direction: column; gap: 4px; }
.sandbox-card-title { font-size: 13px; font-weight: 650; }
.sandbox-card-desc, .sandbox-markdown, .sandbox-keyboard { font-size: 12px; white-space: pre-wrap; }
.sandbox-keyboard { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 6px; }
.sandbox-keyboard span { border: 1px solid var(--border, transparent); border-radius: 8px; padding: 4px 8px; font-size: 11px; }
.sandbox-draft-chips { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.sandbox-draft-chip { display: inline-flex; align-items: center; gap: 4px; border-radius: 999px; padding: 2px 8px; font-size: 11px; background: var(--bg-hover, transparent); }
.sandbox-mention-picker {
  position: absolute; z-index: 1850; left: 0; bottom: calc(100% + 4px);
  min-width: 160px; max-width: 240px; max-height: 160px; overflow: auto;
}
.sandbox-mention-picker[hidden] { display: none; }
.sandbox-sticker-picker {
  position: absolute; z-index: 1850; left: 0; bottom: calc(100% + 4px);
  width: min(320px, 100%); max-height: 220px; overflow: auto;
  display: grid; grid-template-columns: repeat(auto-fill, minmax(56px, 1fr)); gap: 6px;
  padding: 8px; box-sizing: border-box;
}
.sandbox-sticker-picker[hidden] { display: none; }
.sandbox-sticker-picker .sandbox-sticker-add,
.sandbox-sticker-picker .sandbox-sticker-empty { grid-column: 1 / -1; }
.sandbox-sticker-item {
  display: grid; place-items: center; width: 56px; height: 56px; padding: 0;
  border: 0; border-radius: 8px; background: var(--bg-hover, transparent); cursor: pointer; overflow: hidden;
}
.sandbox-sticker-item img { width: 48px; height: 48px; object-fit: cover; border-radius: 8px; }
.sandbox-sticker-face { font-size: 10px; line-height: 1.2; text-align: center; padding: 4px; }
@media (max-width: 960px) {
  .sandbox-frame { grid-template-columns: 1fr; min-height: 0; }
  .sandbox-pane + .sandbox-pane { border-left: 0; border-top: 1px solid var(--border, transparent); }
  .sandbox-session-list { max-height: 220px; }
  .sandbox-member-list { max-height: 180px; }
  .sandbox-messages { min-height: 280px; }
}
`;

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function button(label) {
  const node = element("button", "ghost", label);
  node.type = "button";
  return node;
}

const ICONS = {
  plus: '<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>',
  image: '<rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>',
  paperclip: '<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
  smile: '<circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" y1="9" x2="9.01" y2="9"/><line x1="15" y1="9" x2="15.01" y2="9"/>',
  markdown: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>',
  grid: '<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/>',
  reply: '<polyline points="9 14 4 9 9 4"/><path d="M20 20v-7a4 4 0 0 0-4-4H4"/>',
  close: '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>',
};

function svgIcon(markup) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.innerHTML = markup;
  return svg;
}

function iconButton(title, markup) {
  const node = element("button", "ghost sandbox-add");
  node.type = "button";
  node.title = title;
  node.setAttribute("aria-label", title);
  node.append(svgIcon(markup));
  return node;
}

function liliaInput(type = "text") {
  const input = element("input", "ui-input ui-input--sm");
  input.type = type;
  return input;
}

function liliaTextarea() {
  return element("textarea", "ui-textarea");
}

function menuButton(label, onclick) {
  const node = element("button", "", label);
  node.type = "button";
  node.onclick = onclick;
  return node;
}

function errorMessage(error, fallback = "操作失败，请稍后重试") {
  const raw = error?.message || String(error ?? "");
  const start = raw.indexOf("{");
  if (start >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(start));
      if (typeof parsed.message === "string") return parsed.message;
    } catch (_) {}
  }
  return raw.startsWith("extension ") || raw.includes("rpc ") ? fallback : raw || fallback;
}

function kindLabel(kind) {
  return { group: "群聊", private: "私聊", channel: "频道" }[kind] || kind || "会话";
}

function avatar(name, className = "sandbox-avatar", avatarUrl) {
  const value = String(name || "?").trim();
  const initial = value ? value.slice(0, 1).toUpperCase() : "?";
  if (!avatarUrl) return element("span", className, initial);
  const img = remoteImage(className, value || "头像", avatarUrl);
  img.onerror = () => img.replaceWith(element("span", className, initial));
  return img;
}

function httpsQqCdn(url) {
  const value = String(url || "").trim();
  const href = value.startsWith("//")
    ? `https:${value}`
    : value.slice(0, 7).toLowerCase() === "http://"
      ? `https://${value.slice(7)}`
      : "";
  try {
    return href && /(^|\.)(qlogo\.cn|qpic\.cn|gtimg\.cn|qq\.com\.cn|qq\.com)$/i.test(new URL(href).hostname)
      ? href
      : value;
  } catch (_) {
    return value;
  }
}

function remoteImage(className, alt, src) {
  const img = element("img", className);
  img.alt = alt || "";
  img.setAttribute("referrerpolicy", "no-referrer");
  if (src) img.src = httpsQqCdn(src);
  return img;
}

function userById(users, userId) {
  return (users || []).find((user) => user.user_id === userId);
}

function formatTime(unixMs) {
  const date = new Date(Number(unixMs));
  if (!unixMs || Number.isNaN(date.getTime())) return "";
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString();
}

function quotedText(messages, replyTo) {
  const quoted = (messages || []).find((item) => item.message_id === replyTo);
  if (!quoted) return "引用消息";
  const img = (quoted.refs || []).find((ref) => ref.t === "img");
  const text = stripImagePlaceholder(quoted.text || "", img);
  return `${quoted.sender_name}: ${text || (img ? "[图片]" : "消息")}`;
}

function stripImagePlaceholder(text, ref) {
  return ref?.t === "img" && text.endsWith("[图片]") ? text.slice(0, -"[图片]".length) : text;
}

function mentionName(users, userId) {
  return (users || []).find((user) => user.user_id === userId)?.display_name || userId || "";
}

function botProfile(snapshot) {
  return snapshot?.bot || { user_id: "bot", display_name: "机器人" };
}

function groupConsecutiveMessages(messages) {
  const groups = [];
  for (const message of messages || []) {
    const prev = groups.at(-1)?.[0];
    if (prev && prev.role !== "system" && prev.role === message.role && prev.sender_id === message.sender_id) {
      groups.at(-1).push(message);
    } else groups.push([message]);
  }
  return groups;
}

function arkFields(payload) {
  const kv = payload?.kv || payload?.ark?.kv || [];
  const map = Object.fromEntries((Array.isArray(kv) ? kv : []).map((item) => [item?.key, item?.value || ""]));
  return {
    title: map["#METATITLE#"] || map["#PROMPT#"] || payload?.title || "小卡片",
    desc: map["#METADESC#"] || payload?.description || "",
    image: map["#PIC#"] || map["#METAIMAGE#"] || payload?.image || "",
    url: map["#METAURL#"] || payload?.url || "",
  };
}

function copyText(text) {
  return (navigator.clipboard?.writeText(text) || Promise.reject()).catch(() => {
    const input = element("textarea");
    input.value = text;
    document.body.append(input);
    input.select();
    try { if (!document.execCommand("copy")) throw new Error("复制失败"); }
    finally { input.remove(); }
  });
}

const mediaUrls = new Map();

function bindBlob(node, id, method, key, rpc) {
  if (!id) return;
  const cacheKey = `${method}:${id}`;
  if (mediaUrls.has(cacheKey)) {
    node.src = mediaUrls.get(cacheKey);
    return;
  }
  void rpc.read("sandbox", method, { [key]: id }).then((blob) => {
    const binary = Uint8Array.from(atob(blob.bytes || ""), (ch) => ch.charCodeAt(0));
    const url = URL.createObjectURL(new Blob([binary], { type: blob.mime || "application/octet-stream" }));
    mediaUrls.set(cacheKey, url);
    node.src = url;
  }).catch(() => {
    node.replaceWith(document.createTextNode("[媒体不可用]"));
  });
}

function bindMedia(node, mediaId, rpc) {
  bindBlob(node, mediaId, "media.get", "media_id", rpc);
}

function bindSticker(node, stickerId, rpc) {
  bindBlob(node, stickerId, "sticker.get", "sticker_id", rpc);
}

function renderSegments(message, messages, rpc) {
  const wrap = document.createDocumentFragment();
  if (message.reply_to) wrap.append(element("p", "sandbox-quote muted", quotedText(messages, message.reply_to)));
  const body = element("div", "sandbox-rich");
  renderRefs(body, message, rpc);
  if (!body.textContent && !body.querySelector("img, audio, video, .sandbox-card") && !message.reply_to) {
    body.textContent = message.text || "";
  }
  wrap.append(body);
  return wrap;
}

function renderRefs(body, message, rpc) {
  const chars = [...(message.text || "")];
  let cursor = 0;
  const refs = (message.refs || []).slice().sort((left, right) => (left.at || 0) - (right.at || 0));
  refs.forEach((ref) => {
    const at = Math.min(ref.at || 0, chars.length);
    if (at > cursor) {
      const plain = stripImagePlaceholder(chars.slice(cursor, at).join(""), ref);
      if (plain) body.append(document.createTextNode(plain));
    }
    if (ref.t === "mention" || ref.t === "mention_all") {
      const length = ref.t === "mention_all" ? [..."@全体成员"].length : 1 + [...(ref.name || "")].length;
      body.append(element("span", "sandbox-mention", chars.slice(at, at + length).join("") || `@${ref.name || ref.id || ""}`));
      cursor = at + length;
      return;
    }
    cursor = at;
    appendRef(body, ref, rpc);
  });
  if (cursor < chars.length) {
    const rest = chars.slice(cursor).join("");
    if (!(rest === "[图片]" && (message.refs || []).some((ref) => ref.t === "img"))) {
      body.append(document.createTextNode(rest));
    }
  }
}

function appendRef(body, ref, rpc) {
  if (ref.t === "emoji") {
    body.append(element("span", "sandbox-file", "[表情]"));
    return;
  }
  if (ref.t === "sticker") {
    const img = remoteImage("sandbox-media", ref.name || "表情包");
    if (ref.h) bindSticker(img, ref.h, rpc);
    body.append(img);
    return;
  }
  if (ref.t === "img" || ref.t === "audio" || ref.t === "video" || ref.t === "file") {
    renderPlatform(body, {
      kind: ref.url ? "attachment" : "media",
      payload: { url: ref.url, media_id: ref.h, mime: ref.mime, content_type: ref.mime, name: ref.name, filename: ref.name },
    }, rpc);
    return;
  }
  if (ref.t === "ark" || ref.t === "embed" || ref.t === "markdown" || ref.t === "keyboard") {
    renderPlatform(body, { kind: ref.t === "embed" ? "embed" : ref.t, payload: ref.p || {} }, rpc);
    return;
  }
  body.append(document.createTextNode(`[${ref.t}]`));
}

function renderPlatform(body, segment, rpc) {
  const kind = segment.kind || "";
  const payload = segment.payload || {};
  if (kind === "media" || kind === "attachment") {
    const mime = payload.mime || payload.content_type || "";
    const url = payload.url;
    const mediaId = payload.media_id;
    if (mime.startsWith("audio/") || mime.startsWith("video/")) {
      const node = document.createElement(mime.startsWith("audio/") ? "audio" : "video");
      node.className = "sandbox-media";
      node.controls = true;
      node.setAttribute("referrerpolicy", "no-referrer");
      if (url) node.src = url;
      else if (mediaId) bindMedia(node, mediaId, rpc);
      body.append(node);
      return;
    }
    if (mime.startsWith("image/") || kind === "attachment" || kind === "media") {
      const img = remoteImage("sandbox-media", payload.name || payload.filename || "图片", url);
      if (!url && mediaId) bindMedia(img, mediaId, rpc);
      body.append(img);
      return;
    }
    body.append(element("span", "sandbox-file", `[${payload.name || payload.filename || "文件"}]`));
    return;
  }
  if (kind === "ark" || kind === "embed") {
    const fields = arkFields(payload);
    const card = element("div", "sandbox-card");
    if (fields.image) {
      card.append(remoteImage("", fields.title, fields.image));
    }
    const inner = element("div", "sandbox-card-body");
    inner.append(element("span", "sandbox-card-title", fields.title));
    if (fields.desc) inner.append(element("span", "muted sandbox-card-desc", fields.desc));
    if (fields.url) inner.append(element("span", "muted sandbox-card-desc", fields.url));
    card.append(inner);
    body.append(card);
    return;
  }
  if (kind === "markdown") {
    const markdown = element("pre", "sandbox-markdown");
    markdown.textContent = payload.content || payload.markdown?.content || JSON.stringify(payload);
    body.append(markdown);
    return;
  }
  if (kind === "keyboard") {
    const bar = element("div", "sandbox-keyboard");
    const rows = payload.content?.rows || payload.rows || [];
    rows.flatMap((row) => row.buttons || []).forEach((item) => {
      bar.append(element("span", "", item.render_data?.label || item.label || "按钮"));
    });
    if (!bar.childNodes.length) bar.append(element("span", "", "[按钮]"));
    body.append(bar);
    return;
  }
  body.append(document.createTextNode(`[${kind}]`));
}

/** Mount the QQ sandbox conversation console. */
export function mountSandboxPanel(host, rpc, events) {
  const state = { snapshot: null, messages: [], selectedId: null, draft: "", draftSegments: [], speakerId: "", query: "", quote: null, dialog: "", menu: null, stickerOpen: false, stickers: [], markdown: false };
  host.innerHTML = "";
  const style = document.createElement("style");
  style.textContent = STYLE;
  const status = element("p", "muted sandbox-status");
  status.hidden = true;
  const frame = element("div", "sandbox-frame");
  const root = element("div", "sandbox-panel sandbox-client settings-page stack");
  root.append(frame);
  host.append(style, root);

  const showStatus = (text, kind = "muted") => {
    status.className = `${kind} sandbox-status`;
    status.textContent = text;
    status.hidden = !text;
  };
  const reportError = (error) => showStatus(errorMessage(error), "error-banner");
  const current = () => (state.snapshot?.conversations || []).find((item) => item.conversation_id === state.selectedId) || null;
  const mode = () => state.snapshot?.mode || "simulate";
  const write = (action, confirm = false) => rpc.write("sandbox", "write", {
    confirmed: confirm,
    request: { operation_id: crypto.randomUUID(), expected_revision: state.snapshot?.revision ?? 0, action },
  });

  function renderSessions(pane) {
    const tabs = element("div", "segmented sandbox-mode-tabs");
    const modeSimulate = element("button", "", "模拟");
    const modeLive = element("button", "", "真实数据");
    modeSimulate.type = modeLive.type = "button";
    modeSimulate.classList.toggle("is-active", mode() === "simulate");
    modeLive.classList.toggle("is-active", mode() === "live");
    modeSimulate.onclick = async () => { try { await write({ action: "set_mode", mode: "simulate" }); await refresh(); } catch (error) { reportError(error); } };
    modeLive.onclick = async () => { try { await write({ action: "set_mode", mode: "live" }); await refresh(); } catch (error) { reportError(error); } };
    tabs.append(modeSimulate, modeLive);
    const head = element("div", "sandbox-pane-head");
    head.append(tabs);
    pane.append(head, status);
    const searchRow = element("div", "sandbox-search-row");
    const search = liliaInput("search");
    search.placeholder = "搜索会话";
    search.value = state.query;
    search.oninput = () => { state.query = search.value; render(); };
    searchRow.append(search);
    if (mode() === "simulate") {
      const addUser = iconButton("添加用户", ICONS.plus);
      addUser.onclick = async () => { try { await write({ action: "add_user" }); await refresh(); } catch (error) { reportError(error); } };
      searchRow.append(addUser);
    }
    pane.append(searchRow);
    const list = element("div", "sandbox-session-list");
    const query = state.query.trim().toLowerCase();
    const items = (state.snapshot?.conversations || []).filter((conversation) => !query
      || (conversation.title || "").toLowerCase().includes(query)
      || (conversation.users || []).some((user) => (user.display_name || "").toLowerCase().includes(query)));
    if (!items.length) list.append(element("p", "muted sandbox-empty", "暂无会话"));
    items.forEach((conversation) => {
      const item = element("button", "sandbox-session");
      item.type = "button";
      if (conversation.conversation_id === state.selectedId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(
        element("span", "sandbox-session-title", conversation.title || kindLabel(conversation.kind)),
        element("p", "muted sandbox-session-preview", `${kindLabel(conversation.kind)} · ${conversation.last_preview || "暂无消息"}`),
      );
      const peer = (conversation.users || [])[0];
      item.append(avatar(conversation.title || kindLabel(conversation.kind), "sandbox-avatar", conversation.avatar_url || peer?.avatar_url), meta, element("span", "muted sandbox-session-time", formatTime(conversation.last_activity_unix_ms)));
      item.onclick = () => { state.selectedId = conversation.conversation_id; state.quote = null; void loadConversation(); };
      list.append(item);
    });
    pane.append(list);
  }

  function renderMessages(pane, conversation) {
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", conversation ? conversation.title : "对话"));
    if (conversation) head.append(element("span", "muted", kindLabel(conversation.kind)));
    if (mode() === "simulate" && conversation) {
      const clearChat = button("清空");
      clearChat.classList.add("sandbox-clear");
      clearChat.onclick = async () => {
        try {
          await write({ action: "clear_conversation", conversation_id: conversation.conversation_id });
          await refresh();
        } catch (error) {
          reportError(error);
        }
      };
      head.append(clearChat);
    }
    pane.append(head);
    const messages = element("div", "sandbox-messages");
    if (!conversation) messages.append(element("p", "muted sandbox-empty", "选择一个会话开始"));
    else if (!state.messages.length) messages.append(element("p", "muted sandbox-empty", "暂无消息"));
    else groupConsecutiveMessages(state.messages).forEach((group) => {
      const first = group[0];
      const row = element("div", `sandbox-row sandbox-row--${first.role}`);
      const sender = first.role === "bot" ? botProfile(state.snapshot) : userById(conversation.users, first.sender_id);
      const name = sender?.display_name || first.sender_name;
      if (first.role !== "system") row.append(avatar(name, "sandbox-avatar sandbox-avatar--sm", sender?.avatar_url));
      const body = element("div", "sandbox-row-body");
      group.forEach((message, index) => {
        const bubble = element("div", "sandbox-bubble");
        if (first.role !== "system" && index === 0) {
          bubble.append(element("p", "muted", `${name} · ${formatTime(message.time_ms)}`));
        }
        bubble.append(renderSegments(message, state.messages, rpc));
        if (message.role === "user") {
          const reply = iconButton("回复", ICONS.reply);
          reply.classList.add("sandbox-reply");
          reply.onclick = (event) => {
            event.stopPropagation();
            state.quote = message;
            render();
          };
          bubble.append(reply);
        }
        body.append(bubble);
      });
      row.append(body);
      messages.append(row);
    });
    pane.append(messages);
    if (!conversation) return;
    const compose = element("div", "sandbox-compose");
    if (state.quote) {
      const bar = element("div", "sandbox-quote-bar muted");
      const cancel = button("取消");
      cancel.onclick = () => { state.quote = null; render(); };
      bar.append(element("span", "", `正在回复 ${state.quote.sender_name}`), cancel);
      compose.append(bar);
    }
    if (state.draftSegments.length) {
      const chips = element("div", "sandbox-draft-chips");
      state.draftSegments.forEach((segment, index) => {
        const chip = element("span", "sandbox-draft-chip", draftLabel(segment, conversation.users));
        const remove = iconButton("移除", ICONS.close);
        remove.onclick = () => { state.draftSegments.splice(index, 1); render(); };
        chip.append(remove);
        chips.append(chip);
      });
      compose.append(chips);
    }
    const picker = element("div", "sandbox-context-menu sandbox-mention-picker");
    picker.hidden = true;
    const stickerPicker = element("div", "sandbox-context-menu sandbox-sticker-picker");
    stickerPicker.hidden = !state.stickerOpen;
    if (!stickerPicker.hidden) renderStickerPicker(stickerPicker);
    const row = element("div", "sandbox-compose-row");
    const imageBtn = iconButton("图片", ICONS.image);
    imageBtn.onclick = () => void pickAndUpload("image/*");
    const fileBtn = iconButton("文件", ICONS.paperclip);
    fileBtn.onclick = () => void pickAndUpload("*/*");
    const stickerBtn = iconButton("表情包", ICONS.smile);
    stickerBtn.onclick = () => {
      state.stickerOpen = !state.stickerOpen;
      if (state.stickerOpen) void loadStickers().then(() => render());
      else render();
    };
    row.append(imageBtn, fileBtn, stickerBtn);
    const markdownBtn = iconButton("Markdown", ICONS.markdown);
    if (state.markdown) markdownBtn.classList.add("is-active");
    markdownBtn.onclick = () => {
      state.markdown = !state.markdown;
      if (!state.markdown) {
        state.draftSegments = state.draftSegments.filter((segment) => !isKeyboardSegment(segment));
      }
      render();
    };
    const keyboardBtn = iconButton("消息按钮", ICONS.grid);
    if (state.draftSegments.some(isKeyboardSegment)) keyboardBtn.classList.add("is-active");
    keyboardBtn.onclick = () => {
      if (!state.markdown) state.markdown = true;
      openKeyboardDialog();
    };
    row.append(markdownBtn, keyboardBtn);
    const field = element("div", "sandbox-compose-field");
    const input = state.markdown ? liliaTextarea() : liliaInput("text");
    const canActive = conversation.active_message !== false;
    input.placeholder = state.markdown
      ? "输入 Markdown，Ctrl/Cmd+Enter 发送"
      : mode() === "live"
      ? (canActive ? "可直接发送主动消息，或悬停消息后回复" : "请先悬停用户消息并点击回复")
      : "输入消息，Enter 发送";
    input.value = state.draft;
    const refreshPicker = () => {
      const match = /@([^\s@]*)$/.exec(state.draft);
      picker.replaceChildren();
      if (!match) { picker.hidden = true; return; }
      const query = match[1].toLowerCase();
      const hits = [{ user_id: "__all__", display_name: "全体成员" }, ...(conversation.users || [])]
        .filter((user) => (user.display_name || "").toLowerCase().includes(query) || user.user_id.toLowerCase().includes(query));
      hits.forEach((user) => {
        picker.append(menuButton(`@${user.display_name || user.user_id}`, () => {
          state.draft = state.draft.slice(0, match.index);
          state.draftSegments.push(user.user_id === "__all__"
            ? { type: "mention_all" }
            : { type: "mention_user", user_id: user.user_id });
          render();
        }));
      });
      picker.hidden = !hits.length;
    };
    input.oninput = () => { state.draft = input.value; refreshPicker(); };
    const send = button(mode() === "live" ? "发送到 QQ" : "发送");
    const submit = async () => {
      const payload = buildComposePayload();
      if (!payload) return;
      if (mode() === "live" && !state.quote?.message_id && conversation.active_message === false) {
        showStatus("当前会话没有主动消息权限，请先悬停用户消息并点击回复");
        return;
      }
      try {
        if (mode() === "live") {
          await write({
            action: "send_as_bot",
            conversation_id: conversation.conversation_id,
            text: payload.text,
            segments: payload.segments,
            reply_to: state.quote?.message_id || null,
          }, true);
        } else {
          const speaker = state.speakerId || conversation.users?.[0]?.user_id;
          if (!speaker) { showStatus("请先添加用户"); return; }
          await write({
            action: "ingest_as_user",
            conversation_id: conversation.conversation_id,
            user_id: speaker,
            text: payload.text,
            segments: payload.segments,
            reply_to: state.quote?.message_id || null,
          });
        }
        state.draft = "";
        state.draftSegments = [];
        state.quote = null;
        state.stickerOpen = false;
        state.markdown = false;
        showStatus("");
        await refresh();
      } catch (error) {
        reportError(error);
      }
    };
    send.onclick = () => void submit();
    input.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      if (state.markdown && !event.metaKey && !event.ctrlKey) return;
      event.preventDefault();
      void submit();
    };
    const onPaste = (event) => {
      const image = pasteImage(event);
      if (!image) return;
      event.preventDefault();
      void uploadDraftFile(image);
    };
    input.onpaste = onPaste;
    compose.addEventListener("paste", onPaste);
    field.append(picker, stickerPicker, input);
    row.append(field, send);
    compose.append(row);
    refreshPicker();
    pane.append(compose);
  }

  function pasteImage(event) {
    const files = [...(event.clipboardData?.files || [])];
    const items = [...(event.clipboardData?.items || [])];
    return files.find((file) => file.type.startsWith("image/"))
      || items.find((item) => item.type.startsWith("image/"))?.getAsFile()
      || null;
  }

  async function fileToBase64(file) {
    const buffer = new Uint8Array(await file.arrayBuffer());
    let binary = "";
    buffer.forEach((byte) => { binary += String.fromCharCode(byte); });
    return btoa(binary);
  }

  async function uploadDraftFile(file) {
    try {
      const uploaded = await rpc.write("sandbox", "media.upload", {
        name: file.name || "paste.png",
        mime: file.type || "image/png",
        bytes: await fileToBase64(file),
      });
      state.draftSegments.push({
        type: "platform_specific",
        platform: "sandbox",
        kind: "media",
        payload: { media_id: uploaded.media_id, mime: uploaded.mime, name: uploaded.name },
      });
      showStatus("");
      render();
    } catch (error) {
      reportError(error);
    }
  }

  async function pickAndUpload(accept) {
    const file = await pickFile(accept);
    if (file) await uploadDraftFile(file);
  }

  function pickFile(accept) {
    return new Promise((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = accept;
      input.onchange = () => resolve(input.files?.[0] || null);
      input.click();
    });
  }

  async function loadStickers() {
    try {
      state.stickers = await rpc.read("sandbox", "sticker.list", {}) || [];
    } catch (error) {
      reportError(error);
      state.stickers = [];
    }
  }

  function renderStickerPicker(picker) {
    picker.replaceChildren();
    const add = button("添加");
    add.className = "sandbox-sticker-add";
    add.onclick = async () => {
      const file = await pickFile("image/*");
      if (!file) return;
      try {
        await rpc.write("sandbox", "sticker.upload", {
          name: file.name || "sticker.png",
          mime: file.type || "image/png",
          bytes: await fileToBase64(file),
        });
        await loadStickers();
        render();
      } catch (error) {
        reportError(error);
      }
    };
    picker.append(add);
    if (!state.stickers?.length) {
      picker.append(element("p", "muted sandbox-sticker-empty", "还没有表情包"));
      return;
    }
    state.stickers.forEach((item) => {
      if (item.kind === "qq_face" && mode() === "live") return;
      const btn = element("button", "sandbox-sticker-item");
      btn.type = "button";
      if (item.kind === "qq_face") {
        btn.append(element("span", "sandbox-sticker-face", `[表情 ${item.face_type || "?"}:${item.face_id || "?"}]`));
      } else {
        const img = remoteImage("", item.name || "表情包");
        bindSticker(img, item.id, rpc);
        btn.append(img);
      }
      btn.onclick = () => {
        state.draftSegments.push(item.kind === "qq_face"
          ? { type: "platform_specific", platform: "qqbot", kind: "face", payload: { face_type: item.face_type || "", face_id: item.face_id || "" } }
          : { type: "platform_specific", platform: "sandbox", kind: "sticker", payload: { sticker_id: item.id, mime: item.mime, name: item.name } });
        state.stickerOpen = false;
        render();
      };
      picker.append(btn);
    });
  }

  function draftLabel(segment, users) {
    if (segment.type === "mention_user") return `@${mentionName(users, segment.user_id)}`;
    if (segment.type === "mention_all") return "@全体成员";
    if (segment.type === "markdown") return "[Markdown]";
    if (segment.type === "platform_specific" && segment.kind === "media") return `[${segment.payload?.name || "媒体"}]`;
    if (segment.type === "platform_specific" && segment.kind === "sticker") return `[${segment.payload?.name || "表情包"}]`;
    if (segment.type === "platform_specific" && segment.kind === "face") return "[表情]";
    if (segment.type === "platform_specific" && segment.kind === "keyboard") return "[按钮]";
    return segment.type || "附件";
  }

  function isKeyboardSegment(segment) {
    return segment?.type === "platform_specific" && segment.kind === "keyboard";
  }

  function buildComposePayload() {
    const text = state.draft.trim();
    const segments = state.draftSegments.slice();
    const keyboard = segments.some(isKeyboardSegment);
    if (state.markdown) {
      if (segments.some((segment) => segment.type !== "reply" && segment.type !== "quote" && !isKeyboardSegment(segment) && segment.type !== "markdown")) {
        showStatus("Markdown 不能与文本、艾特或媒体混发");
        return null;
      }
      if (!text) {
        showStatus(keyboard ? "按钮需要附在 Markdown 消息上" : "请填写消息");
        return null;
      }
      return { text: "", segments: [{ type: "markdown", content: text }, ...segments.filter(isKeyboardSegment)] };
    }
    if (!text && !segments.length) { showStatus("请填写消息"); return null; }
    if (keyboard) { showStatus("按钮需要附在 Markdown 消息上"); return null; }
    return { text, segments };
  }

  function openKeyboardDialog() {
    state.dialog = "keyboard";
    const existing = root.querySelector(".sandbox-dialog-overlay");
    if (existing?.dataset.dialog === "keyboard") return;
    closeMemberMenu();
    existing?.remove();
    const overlay = element("div", "sandbox-dialog-overlay");
    overlay.dataset.dialog = "keyboard";
    overlay.onclick = (event) => { if (event.target === overlay) closeDialog(); };
    const card = element("div", "sandbox-dialog");
    card.setAttribute("role", "dialog");
    const head = element("div", "sandbox-dialog-head");
    const close = button("关闭");
    close.onclick = () => closeDialog();
    head.append(element("h2", "", "消息按钮"), close);
    const field = element("label", "sandbox-dialog-field", "每行一个按钮，可用 文字 | 指令");
    const area = liliaTextarea();
    const current = state.draftSegments.find(isKeyboardSegment);
    const rows = current?.payload?.content?.rows || [];
    area.value = rows.flatMap((row) => row.buttons || []).map((item) => {
      const label = item.render_data?.label || item.label || "";
      const data = item.action?.data || "";
      return data && data !== label ? `${label} | ${data}` : label;
    }).join("\n");
    field.append(area);
    const body = element("div", "sandbox-dialog-body");
    body.append(field);
    const save = button("保存");
    save.onclick = () => {
      const buttons = area.value.split("\n").flatMap((line, index) => {
        const [label, data] = line.split("|").map((part) => part.trim());
        if (!label) return [];
        return [{
          id: `btn_${index + 1}`,
          render_data: { label, style: 1 },
          action: { type: 2, permission: { type: 2 }, data: data || label, enter: true },
        }];
      });
      state.draftSegments = state.draftSegments.filter((segment) => !isKeyboardSegment(segment));
      if (buttons.length) {
        const packed = [];
        for (let index = 0; index < buttons.length; index += 5) packed.push({ buttons: buttons.slice(index, index + 5) });
        state.draftSegments.push({
          type: "platform_specific",
          platform: "qqbot",
          kind: "keyboard",
          payload: { content: { rows: packed } },
        });
      }
      closeDialog();
      render();
    };
    const foot = element("div", "sandbox-dialog-foot");
    foot.append(save);
    card.append(head, body, foot);
    overlay.append(card);
    root.append(overlay);
  }

  function closeMemberMenu() {
    state.menu?.remove();
    state.menu = null;
  }

  function closeDialog() {
    state.dialog = "";
    root.querySelector(".sandbox-dialog-overlay")?.remove();
    closeMemberMenu();
  }

  function openMemberMenu(event, user) {
    event.preventDefault();
    closeMemberMenu();
    const menu = element("div", "sandbox-context-menu");
    menu.append(menuButton(`@${user.display_name || user.user_id}`, () => {
      closeMemberMenu();
      state.draftSegments.push({ type: "mention_user", user_id: user.user_id });
      render();
    }));
    menu.append(menuButton("复制 OpenID", async () => {
      closeMemberMenu();
      try { await copyText(user.user_id); showStatus("已复制 OpenID"); }
      catch (error) { reportError(error); }
    }));
    if (mode() === "live") {
      menu.append(menuButton("导入作为模拟用户", async () => {
        closeMemberMenu();
        try {
          const written = await write({ action: "import_live_users", user_ids: [user.user_id] });
          if (state.snapshot) state.snapshot.revision = written.revision;
          if (!written.result?.imported?.length) { showStatus("该成员已在模拟花名册中"); return; }
          state.speakerId = user.user_id;
          await write({ action: "set_mode", mode: "simulate" });
          showStatus("已导入为模拟用户");
          await refresh();
        } catch (error) { reportError(error); }
      }));
    }
    const rect = root.getBoundingClientRect();
    menu.style.left = `${event.clientX - rect.left}px`;
    menu.style.top = `${event.clientY - rect.top}px`;
    root.append(menu);
    state.menu = menu;
  }

  function openDialog(dialog) {
    if (mode() !== "simulate") { closeDialog(); return; }
    state.dialog = dialog;
    const existing = root.querySelector(".sandbox-dialog-overlay");
    if (existing?.dataset.dialog === dialog) return;
    existing?.remove();
    const overlay = element("div", "sandbox-dialog-overlay");
    overlay.dataset.dialog = dialog;
    overlay.onclick = () => closeDialog();
    const card = element("div", "sandbox-dialog");
    card.setAttribute("role", "dialog");
    card.setAttribute("aria-modal", "true");
    card.onclick = (event) => event.stopPropagation();
    const head = element("div", "sandbox-dialog-head");
    const body = element("div", "sandbox-dialog-body");
    const foot = element("div", "sandbox-dialog-foot");
    const cancel = button("取消");
    cancel.onclick = () => closeDialog();
    if (dialog !== "import") {
      const user = (state.snapshot?.conversations || []).flatMap((item) => item.users || []).find((item) => item.user_id === dialog);
      if (!user) { closeDialog(); return; }
      card.setAttribute("aria-label", "编辑成员");
      head.append(element("h2", "", "编辑成员"));
      const openid = liliaInput("text");
      openid.placeholder = "OpenID";
      openid.value = user.user_id;
      const nickname = liliaInput("text");
      nickname.placeholder = "昵称";
      nickname.value = user.display_name || "";
      const openidField = element("label", "sandbox-dialog-field", "OpenID");
      const nicknameField = element("label", "sandbox-dialog-field", "昵称");
      openidField.append(openid);
      nicknameField.append(nickname);
      body.append(openidField, nicknameField);
      const save = button("保存");
      save.onclick = async () => {
        const newId = openid.value.trim();
        const wasPrivate = current()?.conversation?.user_id === user.user_id;
        try {
          await write({ action: "update_user", user_id: user.user_id, new_user_id: newId, display_name: nickname.value.trim() });
          if (state.speakerId === user.user_id) state.speakerId = newId;
          closeDialog();
          await refresh();
          if (wasPrivate) {
            const next = (state.snapshot?.conversations || []).find((item) => item.conversation?.user_id === newId);
            if (next) { state.selectedId = next.conversation_id; await loadConversation(); }
          }
        } catch (error) {
          reportError(error);
        }
      };
      foot.append(cancel, save);
      card.append(head, body, foot);
      overlay.append(card);
      root.append(overlay);
      openid.focus();
      openid.select();
      return;
    }
    const liveUsers = state.snapshot?.live_users || [];
    const taken = new Set((current()?.users || []).map((user) => user.user_id));
    card.setAttribute("aria-label", "导入成员");
    head.append(element("h2", "", "导入成员"));
    if (!liveUsers.length) body.append(element("p", "muted sandbox-empty", "真实数据还没有成员可导入"));
    liveUsers.forEach((user) => {
      const item = element("button", "sandbox-member");
      item.type = "button";
      item.disabled = taken.has(user.user_id);
      const meta = element("div", "");
      meta.append(element("span", "sandbox-member-name", user.display_name || user.user_id), element("p", "muted sandbox-session-preview", user.user_id));
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm", user.avatar_url), meta);
      item.onclick = async () => {
        try {
          await write({ action: "import_live_users", user_ids: [user.user_id] });
          closeDialog();
          await refresh();
        } catch (error) {
          reportError(error);
        }
      };
      body.append(item);
    });
    const importAll = button("全部导入");
    importAll.onclick = async () => {
      const user_ids = liveUsers.map((user) => user.user_id).filter((id) => !taken.has(id));
      if (!user_ids.length) { showStatus("没有可导入的成员"); return; }
      try {
        await write({ action: "import_live_users", user_ids });
        closeDialog();
        await refresh();
      } catch (error) {
        reportError(error);
      }
    };
    foot.append(cancel);
    if (liveUsers.length) foot.append(importAll);
    card.append(head, body, foot);
    overlay.append(card);
    root.append(overlay);
  }

  function renderMembers(pane, conversation) {
    if (mode() !== "simulate") closeDialog();
    const head = element("div", "sandbox-pane-head");
    head.append(element("h2", "", "成员"));
    if (mode() === "simulate") {
      const importMembers = button("导入");
      importMembers.classList.add("sandbox-clear");
      importMembers.onclick = () => openDialog("import");
      head.append(importMembers);
    }
    pane.append(head);
    const list = element("div", "sandbox-member-list");
    const roster = conversation?.users || [];
    if (conversation && !roster.length) list.append(element("p", "muted sandbox-empty", "暂无成员"));
    roster.forEach((user) => {
      const item = element("div", "sandbox-member");
      item.setAttribute("role", "button");
      item.tabIndex = 0;
      if (mode() === "simulate" && user.user_id === state.speakerId) item.classList.add("is-active");
      const meta = element("div", "");
      meta.append(element("span", "sandbox-member-name", user.display_name || user.user_id));
      item.oncontextmenu = (event) => openMemberMenu(event, user);
      if (mode() === "simulate") {
        const actions = element("div", "sandbox-member-actions");
        const edit = button("编辑");
        edit.onclick = (event) => { event.stopPropagation(); openDialog(user.user_id); };
        const remove = button("移除");
        remove.onclick = async (event) => {
          event.stopPropagation();
          try {
            await write({ action: "remove_user", user_id: user.user_id });
            if (state.speakerId === user.user_id) state.speakerId = "";
            if (state.dialog === user.user_id) closeDialog();
            await refresh();
          } catch (error) {
            reportError(error);
          }
        };
        actions.append(edit, remove);
        meta.append(actions);
        item.onclick = () => { state.speakerId = user.user_id; render(); };
        item.onkeydown = (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); state.speakerId = user.user_id; render(); } };
      }
      item.append(avatar(user.display_name || user.user_id, "sandbox-avatar sandbox-avatar--sm", user.avatar_url), meta);
      list.append(item);
    });
    pane.append(list);
  }

  function render() {
    closeMemberMenu();
    const sessions = element("section", "sandbox-pane");
    const chat = element("section", "sandbox-pane sandbox-pane--chat");
    const members = element("section", "sandbox-pane");
    renderSessions(sessions);
    renderMessages(chat, current());
    renderMembers(members, current());
    frame.replaceChildren(sessions, chat, members);
    if (state.dialog === "keyboard") openKeyboardDialog();
    else if (state.dialog) openDialog(state.dialog);
  }

  async function loadConversation() {
    if (!state.selectedId) { state.messages = []; render(); return; }
    try {
      state.messages = await rpc.read("sandbox", "messages", { conversation_id: state.selectedId }) || [];
      const conversation = current();
      if (mode() === "simulate" && conversation?.users?.length && !conversation.users.some((user) => user.user_id === state.speakerId)) {
        state.speakerId = conversation.users[0].user_id;
      }
      render();
    } catch (error) {
      reportError(error);
    }
  }

  let disposed = false;
  let pollTimer = null;
  let inFlight = null;
  const schedule = () => {
    clearTimeout(pollTimer);
    if (!disposed && !document.hidden) pollTimer = setTimeout(() => void refresh(), 15_000);
  };
  function refresh() {
    if (disposed) return Promise.resolve();
    if (inFlight) return inFlight;
    inFlight = (async () => {
      try {
        const snapshot = await rpc.read("sandbox", "snapshot", { query: "" });
        if (disposed) return;
        state.snapshot = snapshot;
        if (!state.selectedId || !(snapshot.conversations || []).some((item) => item.conversation_id === state.selectedId)) {
          state.selectedId = snapshot.conversations?.[0]?.conversation_id || null;
        }
        await loadConversation();
      } catch (error) {
        if (!disposed) reportError(error);
      }
    })().finally(() => { inFlight = null; schedule(); });
    return inFlight;
  }

  const onKey = (event) => {
    if (event.key === "Escape") closeDialog();
  };
  const onPointerDown = (event) => {
    if (state.menu && !state.menu.contains(event.target)) closeMemberMenu();
  };
  const visibility = () => { clearTimeout(pollTimer); if (!document.hidden) void refresh(); };
  const eventSubscription = events.subscribe("sandbox.changed", () => { if (!document.hidden) void refresh(); }, "bot.read");
  document.addEventListener("visibilitychange", visibility);
  document.addEventListener("keydown", onKey);
  document.addEventListener("pointerdown", onPointerDown);
  void refresh();
  return {
    refresh,
    destroy() {
      disposed = true;
      closeMemberMenu();
      clearTimeout(pollTimer);
      eventSubscription.dispose();
      document.removeEventListener("visibilitychange", visibility);
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointerDown);
    },
  };
}

export default {
  id: "sandbox",
  setup(ctx) {
    ctx.pages.register({
      id: "sandbox.page", path: "/sandbox", title: "沙盒",
      component: { mount(el) { const panel = mountSandboxPanel(el, ctx.rpc, ctx.events); return { dispose: () => panel.destroy() }; } },
      requiredCapability: "bot.read",
    });
    ctx.navigation.register({
      id: "sandbox.nav", activityId: "sandbox", pageId: "sandbox.page", label: "沙盒", order: 10,
      requiredCapability: "bot.read",
    });
  },
};
