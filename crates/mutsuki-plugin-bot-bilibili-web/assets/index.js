function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function formatError(err) {
  if (err && typeof err === "object" && err.message) return err.message;
  return String(err ?? "unknown error");
}

function kv(label, value) {
  const row = document.createElement("div");
  row.className = "kv-row";
  row.innerHTML = `<span class="muted">${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong>`;
  return row;
}

function section(title) {
  const el = document.createElement("section");
  el.className = "panel";
  const h = document.createElement("h2");
  h.textContent = title;
  el.appendChild(h);
  return el;
}

function button(label, className = "") {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = className || "ghost";
  btn.textContent = label;
  return btn;
}

function field(label, input) {
  const wrap = document.createElement("label");
  wrap.className = "form-field";
  wrap.innerHTML = `<span>${escapeHtml(label)}</span>`;
  wrap.appendChild(input);
  return wrap;
}

function textInput(placeholder = "", value = "") {
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = placeholder;
  input.value = value;
  return input;
}

/**
 * Mount Bilibili management panel into an overview content host.
 * @param {HTMLElement} host
 * @param {{ read: Function, write: Function }} rpc
 */
export function mountBilibiliPanel(host, rpc) {
  host.innerHTML = "";
  const root = document.createElement("div");
  root.className = "bilibili-panel stack";
  host.appendChild(root);

  const statusBox = section("登陆态");
  const qrBox = section("扫码登录");
  const listBox = section("订阅");
  const addBox = section("新增订阅");
  const bindBox = section("自助绑定");
  const msg = document.createElement("div");
  msg.className = "muted";
  root.append(statusBox, qrBox, listBox, addBox, bindBox, msg);

  let pollTimer = null;
  let status = null;

  function setMessage(text, isError = false) {
    msg.className = isError ? "error-banner" : "muted";
    msg.textContent = text || "";
  }

  async function refreshStatus() {
    status = await rpc.read("bilibili", "status");
    statusBox.querySelectorAll(".kv-row, .actions, .hint").forEach((n) => n.remove());
    statusBox.append(
      kv("后端", status.backend || "—"),
      kv("管理可用", status.available ? "是" : "否"),
      kv("Cookie 密钥", status.cookie_secret_key || "—"),
      kv("Cookie 状态", status.cookie_secret_state || "—"),
      kv("内存凭据", status.credential_loaded ? "已加载" : "未加载"),
      kv("订阅数", String(status.subscription_count ?? 0)),
    );
    if (status.reason) {
      const hint = document.createElement("p");
      hint.className = "hint muted";
      hint.textContent = status.reason;
      statusBox.appendChild(hint);
    }
    const actions = document.createElement("div");
    actions.className = "actions";
    const clearBtn = button("清除凭据", "");
    clearBtn.disabled = !status.available;
    clearBtn.onclick = async () => {
      try {
        await rpc.write("bilibili", "credential.clear");
        setMessage("凭据已清除");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    actions.appendChild(clearBtn);
    statusBox.appendChild(actions);

    qrBox.hidden = !status.available;
    addBox.hidden = !status.available;
    bindBox.hidden = !(status.available && status.allow_self_binding);
  }

  async function refreshList() {
    listBox.querySelectorAll(".sub-list, .empty, .preview").forEach((n) => n.remove());
    if (!status?.management_enabled) {
      listBox.appendChild(Object.assign(document.createElement("p"), {
        className: "empty muted",
        textContent: "管理未启用",
      }));
      return;
    }
    const body = await rpc.read("bilibili", "subscriptions.list", { is_admin: true });
    const items = body?.subscriptions || [];
    if (!items.length) {
      listBox.appendChild(Object.assign(document.createElement("p"), {
        className: "empty muted",
        textContent: "暂无订阅",
      }));
      return;
    }
    const list = document.createElement("div");
    list.className = "sub-list stack";
    for (const item of items) {
      const card = document.createElement("div");
      card.className = "panel nested";
      card.append(
        kv("ID", item.subscription_id),
        kv("UID", String(item.uid)),
        kv(
          "通知",
          (item.notifications || [])
            .map((n) => String(n).toLowerCase())
            .join(", "),
        ),
        kv("绑定", item.outbound_binding),
        kv("暂停", item.paused ? "是" : "否"),
        kv("所有者", item.owner_user_id || "—"),
      );
      const actions = document.createElement("div");
      actions.className = "actions";
      const pauseBtn = button(item.paused ? "恢复" : "暂停");
      pauseBtn.disabled = !status.available;
      pauseBtn.onclick = async () => {
        try {
          await rpc.write("bilibili", "subscriptions.set_paused", {
            selector: item.subscription_id,
            paused: !item.paused,
            is_admin: true,
          });
          await refreshAll();
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      const previewBtn = button("预览");
      previewBtn.onclick = async () => {
        try {
          const cardView = await rpc.read("bilibili", "subscriptions.preview", {
            selector: item.subscription_id,
            is_admin: true,
          });
          let preview = listBox.querySelector(".preview");
          if (!preview) {
            preview = document.createElement("div");
            preview.className = "preview panel nested";
            listBox.appendChild(preview);
          }
          preview.innerHTML = `<strong>${escapeHtml(cardView.title)}</strong>
            <div class="muted">${escapeHtml(cardView.description)}</div>
            <a href="${escapeHtml(cardView.url)}" target="_blank" rel="noreferrer">${escapeHtml(cardView.url)}</a>`;
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      const delBtn = button("删除");
      delBtn.disabled = !status.available;
      delBtn.onclick = async () => {
        try {
          await rpc.write("bilibili", "subscriptions.unsubscribe", {
            subscription_id: item.subscription_id,
          });
          await refreshAll();
        } catch (err) {
          setMessage(formatError(err), true);
        }
      };
      actions.append(pauseBtn, previewBtn, delBtn);
      card.appendChild(actions);
      list.appendChild(card);
    }
    listBox.appendChild(list);
  }

  function buildQrUi() {
    qrBox.querySelectorAll(".qr-body, .actions").forEach((n) => n.remove());
    const body = document.createElement("div");
    body.className = "qr-body";
    const img = document.createElement("img");
    img.alt = "Bilibili 登录二维码";
    img.hidden = true;
    img.style.maxWidth = "256px";
    const state = document.createElement("p");
    state.className = "muted";
    state.textContent = "点击开始扫码登录";
    body.append(img, state);
    const actions = document.createElement("div");
    actions.className = "actions";
    const startBtn = button("开始扫码登录", "");
    startBtn.onclick = async () => {
      try {
        if (pollTimer) clearInterval(pollTimer);
        const started = await rpc.write("bilibili", "login.start");
        img.src = `data:image/png;base64,${started.qr_png_base64}`;
        img.hidden = false;
        state.textContent = "等待扫码…";
        pollTimer = setInterval(async () => {
          try {
            const polled = await rpc.read("bilibili", "login.poll");
            state.textContent = polled.message || polled.status;
            if (polled.status === "confirmed" || polled.status === "expired") {
              clearInterval(pollTimer);
              pollTimer = null;
              if (polled.status === "confirmed") {
                img.hidden = true;
                await refreshAll();
              }
            }
          } catch (err) {
            clearInterval(pollTimer);
            pollTimer = null;
            setMessage(formatError(err), true);
          }
        }, 2000);
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    actions.appendChild(startBtn);
    qrBox.append(body, actions);
  }

  function buildAddForm() {
    addBox.querySelectorAll("form").forEach((n) => n.remove());
    const form = document.createElement("form");
    form.className = "stack";
    const idInput = textInput("subscription_id");
    const uidInput = textInput("uid");
    const bindingInput = textInput("outbound_binding");
    const groupInput = textInput("group_id");
    const notifyInput = textInput("live,dynamic,video", "live,dynamic,video");
    form.append(
      field("订阅 ID", idInput),
      field("UID", uidInput),
      field("出站绑定", bindingInput),
      field("群 ID (BotTarget.group)", groupInput),
      field("通知类型", notifyInput),
    );
    const submit = button("创建订阅", "");
    submit.onclick = async (event) => {
      event.preventDefault();
      try {
        const notifications = notifyInput.value
          .split(",")
          .map((v) => v.trim())
          .filter(Boolean);
        await rpc.write("bilibili", "subscriptions.subscribe", {
          subscription_id: idInput.value.trim(),
          uid: Number(uidInput.value),
          outbound_binding: bindingInput.value.trim(),
          notifications,
          target: { type: "group", group_id: groupInput.value.trim() },
        });
        setMessage("订阅已写入");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    form.appendChild(submit);
    addBox.appendChild(form);
  }

  function buildBindForm() {
    bindBox.querySelectorAll("form, .bind-result").forEach((n) => n.remove());
    const form = document.createElement("form");
    form.className = "stack";
    const operatorInput = textInput("operator_user_id");
    const uidInput = textInput("uid");
    const groupInput = textInput("group_id");
    const result = document.createElement("div");
    result.className = "bind-result muted";
    form.append(
      field("操作者用户 ID", operatorInput),
      field("B 站 UID", uidInput),
      field("验证成功后推送群 ID", groupInput),
    );
    const startBtn = button("发起绑定", "");
    startBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        const challenge = await rpc.write("bilibili", "binding.start", {
          operator_user_id: operatorInput.value.trim(),
          uid: Number(uidInput.value),
        });
        result.textContent = `请把 ${challenge.code} 写入 ${challenge.name} 的个性签名，然后点验证。`;
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    const verifyBtn = button("验证绑定", "");
    verifyBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        const verified = await rpc.write("bilibili", "binding.verify", {
          operator_user_id: operatorInput.value.trim(),
          platform: "web",
          target: { type: "group", group_id: groupInput.value.trim() },
        });
        if (verified.result === "signature_mismatch") {
          result.textContent = `验证未通过：个性签名中尚未找到 ${verified.code}`;
        } else {
          result.textContent = "绑定成功";
          await refreshAll();
        }
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    const unbindBtn = button("解除自助绑定");
    unbindBtn.onclick = async (event) => {
      event.preventDefault();
      try {
        await rpc.write("bilibili", "binding.unbind", {
          operator_user_id: operatorInput.value.trim(),
        });
        setMessage("已解除绑定");
        await refreshAll();
      } catch (err) {
        setMessage(formatError(err), true);
      }
    };
    form.append(startBtn, verifyBtn, unbindBtn, result);
    bindBox.appendChild(form);
  }

  async function refreshAll() {
    setMessage("");
    await refreshStatus();
    buildQrUi();
    buildAddForm();
    buildBindForm();
    await refreshList();
  }

  refreshAll().catch((err) => setMessage(formatError(err), true));
  return {
    destroy() {
      if (pollTimer) clearInterval(pollTimer);
    },
  };
}

export default {
  id: "bilibili",
  setup(ctx) {
    ctx.navigation.register({
      id: "bilibili.nav",
      label: "B站推送",
      path: "/bilibili",
      order: 8,
      requiredCapability: "runtime.read",
    });
    ctx.pages.register({
      id: "bilibili.page",
      path: "/bilibili",
      title: "B站推送",
      component: {
        mount(el) {
          mountBilibiliPanel(el, ctx.rpc);
        },
      },
      requiredCapability: "runtime.read",
    });
  },
};
