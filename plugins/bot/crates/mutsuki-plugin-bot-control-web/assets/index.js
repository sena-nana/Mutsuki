const RECONCILE_MS = 60_000;
const EVENT_DEBOUNCE_MS = 50;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function formatError(error) {
  const message = error && typeof error === "object" && "message" in error
    ? String(error.message)
    : "";
  return message.startsWith("extension ") || message.includes("rpc ")
    ? "操作失败，请稍后重试"
    : message || "操作失败，请稍后重试";
}

function runtimeStateLabel(value) {
  switch (String(value || "").toLowerCase()) {
    case "running":
    case "healthy":
    case "ok":
      return "运行中";
    case "starting":
      return "启动中";
    case "stopping":
      return "停止中";
    case "stopped":
      return "已停止";
    case "failed":
    case "unhealthy":
      return "异常";
    case "degraded":
      return "降级";
    default:
      return value == null || value === "" ? "—" : String(value);
  }
}

function confirmAction(message) {
  return globalThis.confirm?.(message) !== false;
}

function confirmDestructive(label, token) {
  if (!confirmAction(`即将执行 ${label}，是否继续？`)) return false;
  return (globalThis.prompt?.(`请输入 ${token} 以确认`) || "").trim() === token;
}

function empty(text) {
  return `<div class="muted">${escapeHtml(text)}</div>`;
}

function mountSnapshotPage(host, { events, domains, load, render }) {
  host.innerHTML = `
    <div class="toolbar row-item">
      <button type="button" class="ghost" data-refresh>刷新</button>
      <span class="muted" data-refresh-state></span>
    </div>
    <div data-snapshot-body><div class="muted">加载中…</div></div>
  `;
  const body = host.querySelector("[data-snapshot-body]");
  const status = host.querySelector("[data-refresh-state]");
  let disposed = false;
  let timer = null;
  let debounceTimer = null;
  let inFlight = null;
  let pending = false;
  let lastRevision = 0;
  let opened = false;

  const schedule = () => {
    clearTimeout(timer);
    if (!disposed && !document.hidden) timer = setTimeout(refresh, RECONCILE_MS);
  };
  const setStatus = (message, isError = false) => {
    if (disposed) return;
    status.className = isError ? "err-text" : "muted";
    status.textContent = message;
  };
  const refresh = () => {
    if (disposed) return Promise.resolve();
    pending = true;
    if (inFlight) return inFlight;
    inFlight = (async () => {
      while (pending && !disposed) {
        pending = false;
        try {
          const value = await load();
          if (disposed) return;
          render(body, value, { refresh, setStatus });
          setStatus("");
        } catch {
          setStatus("更新失败，将自动重试", true);
        }
      }
    })().finally(() => {
      inFlight = null;
      schedule();
    });
    return inFlight;
  };
  const eventSubscription = events.subscribe("control.changed", (payload) => {
    const revision = Number(payload?.revision || 0);
    const changed = Array.isArray(payload?.domains) ? payload.domains : [];
    if (revision <= lastRevision || !changed.some((domain) => domains.includes(domain))) return;
    lastRevision = revision;
    if (document.hidden) return;
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => void refresh(), EVENT_DEBOUNCE_MS);
  }, "runtime.read");
  const connectionSubscription = events.onStateChange?.((state) => {
    if (state !== "open" || document.hidden) return;
    if (opened) void refresh();
    opened = true;
  });
  const visibility = () => {
    clearTimeout(timer);
    if (!document.hidden) void refresh();
  };
  host.querySelector("[data-refresh]").onclick = () => void refresh();
  document.addEventListener("visibilitychange", visibility);
  void refresh();
  return {
    refresh,
    dispose() {
      disposed = true;
      clearTimeout(timer);
      clearTimeout(debounceTimer);
      eventSubscription.dispose();
      connectionSubscription?.dispose();
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}

function mountPlugins(host, rpc, events) {
  return mountSnapshotPage(host, {
    events,
    domains: ["plugins"],
    load: () => rpc.read("control", "plugin_list"),
    render(body, value, controls) {
      const plugins = value?.plugins || [];
      const diagnostics = value?.diagnostics || [];
      body.innerHTML = `
        <div class="toolbar"><button type="button" class="ghost" data-reload>重载插件</button></div>
        ${diagnostics.length ? `<section class="card"><h2>清单诊断</h2>${diagnostics.map((item) => `<div class="tree-item"><strong>${escapeHtml(item.plugin_id || item.manifest_path || "清单")}</strong><div class="muted">${escapeHtml(item.detail || "—")}</div></div>`).join("")}</section>` : ""}
        <div data-plugin-list>${plugins.length ? "" : empty("暂无插件")}</div>
      `;
      const list = body.querySelector("[data-plugin-list]");
      for (const plugin of plugins) {
        const card = document.createElement("section");
        card.className = "card";
        card.innerHTML = `<h2>${escapeHtml(plugin.plugin_id)}</h2><p class="muted">当前部署：${escapeHtml(plugin.active_deployment || "—")}</p>`;
        for (const candidate of plugin.candidates || []) {
          const row = document.createElement("div");
          row.className = "tree-item row-item";
          row.innerHTML = `<div><strong>${escapeHtml(candidate.deployment)}</strong><div class="muted">${escapeHtml(candidate.version || "—")} · ${candidate.available ? "可用" : "不可用"}</div></div><button type="button" class="ghost">设为偏好</button>`;
          const button = row.querySelector("button");
          button.disabled = !candidate.available;
          button.onclick = async () => {
            if (!confirmAction(`将 ${plugin.plugin_id} 的部署偏好设为 ${candidate.deployment}？`)) return;
            try {
              await rpc.write("control", "plugin_deployment_set", {
                plugin_id: plugin.plugin_id,
                deployment: candidate.deployment,
              });
              await controls.refresh();
            } catch (error) {
              controls.setStatus(formatError(error), true);
            }
          };
          card.appendChild(row);
        }
        if (plugin.preferred_deployment) {
          const clear = document.createElement("button");
          clear.type = "button";
          clear.className = "ghost";
          clear.textContent = "清除部署偏好";
          clear.onclick = async () => {
            try {
              await rpc.write("control", "plugin_deployment_clear", { plugin_id: plugin.plugin_id });
              await controls.refresh();
            } catch (error) {
              controls.setStatus(formatError(error), true);
            }
          };
          card.appendChild(clear);
        }
        list.appendChild(card);
      }
      body.querySelector("[data-reload]").onclick = async () => {
        if (!confirmAction("确认重载全部插件？")) return;
        try {
          await rpc.write("control", "plugin_reload");
          await controls.refresh();
        } catch (error) {
          controls.setStatus(formatError(error), true);
        }
      };
    },
  });
}

function mountRunners(host, rpc, events) {
  return mountSnapshotPage(host, {
    events,
    domains: ["runners"],
    load: () => rpc.read("control", "runner_list"),
    render(body, runners, controls) {
      body.replaceChildren();
      if (!runners?.length) {
        body.innerHTML = empty("暂无运行器");
        return;
      }
      for (const runner of runners) {
        const row = document.createElement("div");
        row.className = "tree-item row-item";
        row.innerHTML = `<div><strong>${escapeHtml(runner.runner_id)}</strong><div class="muted">${escapeHtml(runner.plugin_id)} · ${escapeHtml(runtimeStateLabel(runner.state))} · 进程 ${escapeHtml(runner.pid ?? "—")} · 重启 ${escapeHtml(runner.restarts ?? 0)}</div></div><div class="row-actions"><button type="button" class="ghost" data-action="restart">重启</button><button type="button" class="ghost danger" data-action="stop">停止</button></div>`;
        row.querySelectorAll("[data-action]").forEach((button) => {
          button.onclick = async () => {
            const action = button.dataset.action;
            if (!confirmAction(`确认${action === "restart" ? "重启" : "停止"}运行器 ${runner.runner_id}？`)) return;
            try {
              await rpc.write("control", action === "restart" ? "runner_restart" : "runner_stop", { id: runner.runner_id });
              await controls.refresh();
            } catch (error) {
              controls.setStatus(formatError(error), true);
            }
          };
        });
        body.appendChild(row);
      }
    },
  });
}

function mountEventSources(host, rpc, events) {
  return mountSnapshotPage(host, {
    events,
    domains: ["event_sources"],
    load: () => rpc.read("control", "event_source_list"),
    render(body, sources, controls) {
      body.replaceChildren();
      if (!sources?.length) {
        body.innerHTML = empty("暂无事件源");
        return;
      }
      for (const source of sources) {
        const row = document.createElement("div");
        row.className = "tree-item row-item";
        row.innerHTML = `<div><strong>${escapeHtml(source.source_id)}</strong><div class="muted">${escapeHtml(source.plugin_id)} · ${escapeHtml(runtimeStateLabel(source.state))}/${escapeHtml(runtimeStateLabel(source.health))} · 重连 ${escapeHtml(source.reconnects ?? 0)}</div>${source.last_error ? `<div class="err-text">${escapeHtml(source.last_error)}</div>` : ""}</div><button type="button" class="ghost">重启</button>`;
        row.querySelector("button").onclick = async () => {
          if (!confirmAction(`确认重启事件源 ${source.source_id}？`)) return;
          try {
            await rpc.write("control", "event_source_restart", { id: source.source_id });
            await controls.refresh();
          } catch (error) {
            controls.setStatus(formatError(error), true);
          }
        };
        body.appendChild(row);
      }
    },
  });
}

function mountTopology(host, rpc, events) {
  return mountSnapshotPage(host, {
    events,
    domains: ["plugins", "runners", "event_sources"],
    load: async () => {
      const [plugins, runners, sources] = await Promise.all([
        rpc.read("control", "plugin_list"),
        rpc.read("control", "runner_list"),
        rpc.read("control", "event_source_list"),
      ]);
      return { plugins: plugins?.plugins || [], runners: runners || [], sources: sources || [] };
    },
    render(body, value) {
      const nodes = new Map();
      const node = (id) => {
        if (!nodes.has(id)) nodes.set(id, { runners: [], sources: [] });
        return nodes.get(id);
      };
      value.plugins.forEach((item) => node(item.plugin_id));
      value.runners.forEach((item) => node(item.plugin_id).runners.push(item));
      value.sources.forEach((item) => node(item.plugin_id).sources.push(item));
      body.innerHTML = nodes.size
        ? `<div class="topology-graph">${[...nodes]
            .map(
              ([id, item]) => `<section class="topology-node card"><h2>${escapeHtml(id)}</h2><div class="topology-node__lanes"><div class="topology-lane"><h3>运行器</h3>${item.runners.map((runner) => `<div class="topology-chip"><strong>${escapeHtml(runner.runner_id)}</strong><span class="muted">${escapeHtml(runtimeStateLabel(runner.state))}</span></div>`).join("") || empty("无")}</div><div class="topology-lane"><h3>事件源</h3>${item.sources.map((source) => `<div class="topology-chip"><strong>${escapeHtml(source.source_id)}</strong><span class="muted">${escapeHtml(runtimeStateLabel(source.health || source.state))}</span></div>`).join("") || empty("无")}</div></div></section>`,
            )
            .join("")}</div>`
        : empty("暂无拓扑数据");
    },
  });
}

function mountLifecycle(host, rpc) {
  host.innerHTML = `
    <div class="toolbar row-item"><span class="muted" data-state></span></div>
    <section class="card"><h2>排空任务</h2><p class="muted">停止接受新任务，并等待进行中的任务完成。</p><button type="button" class="ghost" data-drain>开始排空</button></section>
    <section class="card"><h2>关闭服务</h2><p class="muted">保存状态后关闭服务。</p><button type="button" class="ghost danger" data-shutdown>关闭服务</button></section>
  `;
  const status = host.querySelector("[data-state]");
  const setStatus = (message, isError = false) => {
    status.className = isError ? "err-text" : "muted";
    status.textContent = message;
  };
  host.querySelector("[data-drain]").onclick = async () => {
    if (!confirmDestructive("排空任务", "排空")) return;
    try {
      await rpc.write("control", "core_begin_drain");
      setStatus("已开始排空");
    } catch (error) {
      setStatus(formatError(error), true);
    }
  };
  host.querySelector("[data-shutdown]").onclick = async () => {
    if (!confirmDestructive("关闭服务", "关闭")) return;
    try {
      await rpc.write("control", "service_shutdown");
      setStatus("关闭信号已发送");
    } catch (error) {
      setStatus(formatError(error), true);
    }
  };
}

function mountLogs(host, rpc, events) {
  let cursor = null;
  let entries = [];
  return mountSnapshotPage(host, {
    events,
    domains: ["logs"],
    load: async () => {
      const result = await rpc.read("control", "log_tail", { cursor, lines: cursor == null ? 50 : 500 });
      if (cursor != null && Number(result?.cursor ?? 0) < cursor) entries = [];
      cursor = Number(result?.cursor ?? 0);
      const offsets = new Set(entries.map((entry) => entry.offset));
      entries.push(...(result?.entries || []).filter((entry) => !offsets.has(entry.offset)));
      entries = entries.slice(-500);
      return entries;
    },
    render(body, lines) {
      body.innerHTML = `<section class="card"><h2>日志尾部</h2>${lines.length ? `<pre class="log-block">${lines.map((entry) => escapeHtml(entry.line)).join("\n")}</pre>` : empty("暂无日志")}</section>`;
    },
  });
}

function mountTasks(host, rpc, events) {
  host.innerHTML = `
    <div class="toolbar row-item"><button type="button" class="ghost" data-refresh>刷新</button><span class="muted" data-state></span></div>
    <div class="tasks-layout"><section class="card"><h2>任务表</h2><div data-table>加载中…</div></section><section class="card"><h2>任务详情</h2><div data-detail class="muted">选择任务</div></section></div>
    <section class="card"><h2>事件时间线</h2><p class="muted">按任务聚焦当前窗口中的运行事件。无时间戳时按序号投影。</p><div data-trajectory class="trajectory"></div></section>
  `;
  const tableHost = host.querySelector("[data-table]");
  const detailHost = host.querySelector("[data-detail]");
  const trajHost = host.querySelector("[data-trajectory]");
  const status = host.querySelector("[data-state]");
  let selectedTaskId = null;
  let disposed = false;
  let timer = null;
  let debounceTimer = null;
  let inFlight = null;
  let pending = false;
  let lastRevision = 0;
  let opened = false;
  let traj = null;
  let loadedEvents = [];
  let earliestAvailable = 0;
  let pageLimit = 100;

  async function ensureTrajectory() {
    if (traj) return traj;
    const [
      { mountTrajectoryView },
      { projectRuntimeEvents, mergeBySequence, sequenceOf, trimWindow, EVENT_WINDOW_LIMIT },
    ] = await Promise.all([
      import("./trajectory-view.js"),
      import("./trajectory-model.js"),
    ]);
    if (disposed) return null;
    function project() {
      return projectRuntimeEvents(loadedEvents, { focusSubjectId: selectedTaskId });
    }
    function firstSequence() {
      const values = loadedEvents.map(sequenceOf).filter((value) => value != null);
      return values.length ? Math.min(...values) : earliestAvailable + 1;
    }
    traj = mountTrajectoryView(trajHost, {
      ...project(),
      hasOlder: false,
      emptyText: "当前窗口没有该任务的事件",
      onInspect: async (record) =>
        loadedEvents.filter((event) => (record.sequences || []).includes(sequenceOf(event))),
      onLoadOlder: async () => {
        await loadTaskEvents({ reset: false });
      },
    });
    traj._project = project;
    traj._mergeBySequence = mergeBySequence;
    traj._sequenceOf = sequenceOf;
    traj._firstSequence = firstSequence;
    traj._trimWindow = trimWindow;
    traj._windowLimit = EVENT_WINDOW_LIMIT;
    return traj;
  }

  async function loadTaskEvents({ reset }) {
    const view = await ensureTrajectory();
    if (!view || disposed) return;
    try {
      if (reset) {
        loadedEvents = [];
        view.update({ loading: true, records: [], groups: [], emptyText: "正在加载时间线…" });
      } else {
        view.update({ loadingOlder: true });
      }
      const probe = reset
        ? await rpc.read("control", "task_events_after", { sequence: 0, limit: 1 })
        : null;
      if (disposed) return;
      if (probe) {
        earliestAvailable = Number(probe.earliest_available_sequence || 0);
        const latest = Number(probe.latest_sequence || 0);
        pageLimit = 100;
        const after = Math.max(
          Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
          latest > pageLimit ? latest - pageLimit : Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
        );
        const page = await rpc.read("control", "task_events_after", {
          sequence: after,
          limit: pageLimit,
        });
        if (disposed) return;
        loadedEvents = page?.events || page?.items || [];
        view.update({
          ...view._project(),
          loading: false,
          loadingOlder: false,
          hasOlder: view._firstSequence() > (earliestAvailable || 1),
          lost: Number(page?.lost || probe.lost || 0),
          dropped: Number(page?.dropped || probe.dropped || 0),
          truncated: !!(page?.has_more || page?.truncated),
          emptyText: "当前窗口没有该任务的事件",
        });
        return;
      }
      const first = view._firstSequence();
      if (first <= (earliestAvailable || 1)) {
        view.update({ loadingOlder: false, hasOlder: false });
        return;
      }
      const after = Math.max(
        Math.max(0, earliestAvailable ? earliestAvailable - 1 : 0),
        first - pageLimit - 1,
      );
      const page = await rpc.read("control", "task_events_after", {
        sequence: after,
        limit: pageLimit,
      });
      if (disposed) return;
      const older = (page?.events || page?.items || []).filter((event) => {
        const seq = view._sequenceOf(event);
        return seq != null && seq < first;
      });
      loadedEvents = view._trimWindow(
        view._mergeBySequence(loadedEvents, older),
        view._windowLimit,
        "older",
      );
      view.update({
        ...view._project(),
        loadingOlder: false,
        hasOlder: view._firstSequence() > (earliestAvailable || 1),
        lost: Number(page?.lost || 0),
        dropped: Number(page?.dropped || 0),
        truncated: !!(page?.has_more || page?.truncated),
      });
    } catch (error) {
      if (!disposed) {
        view.update({
          loading: false,
          loadingOlder: false,
          emptyText: formatError(error),
        });
      }
    }
  }

  const renderTasks = (tasks) => {
    if (!tasks.length) {
      tableHost.innerHTML = empty("暂无任务");
      detailHost.innerHTML = empty("选择任务");
      void loadTaskEvents({ reset: true });
      return;
    }
    const selected = tasks.find((task) => task.task_id === selectedTaskId) || tasks[0];
    selectedTaskId = selected.task_id;
    tableHost.innerHTML = `<table class="data-table"><thead><tr><th>任务</th><th>协议</th><th>状态</th><th>运行器</th></tr></thead><tbody>${tasks.map((task) => `<tr data-task-id="${escapeHtml(task.task_id)}" class="${task.task_id === selectedTaskId ? "is-selected" : ""}"><td class="mono">${escapeHtml(task.task_id)}</td><td>${escapeHtml(task.protocol_id)}</td><td>${escapeHtml(task.status)}</td><td>${escapeHtml(task.runner_hint || "—")}</td></tr>`).join("")}</tbody></table>`;
    tableHost.querySelectorAll("[data-task-id]").forEach((row) => {
      row.onclick = () => {
        selectedTaskId = row.dataset.taskId;
        renderTasks(tasks);
      };
    });
    detailHost.innerHTML = `<ul class="kv"><li><span>任务</span><span class="mono">${escapeHtml(selected.task_id)}</span></li><li><span>协议</span><span>${escapeHtml(selected.protocol_id)}</span></li><li><span>状态</span><span>${escapeHtml(selected.status)}</span></li><li><span>运行器</span><span>${escapeHtml(selected.owner_runner || selected.runner_hint || "—")}</span></li></ul><button type="button" class="ghost" data-cancel>取消任务</button>`;
    detailHost.querySelector("[data-cancel]").onclick = async () => {
      if (!confirmAction(`确认取消任务 ${selected.task_id}？`)) return;
      try {
        await rpc.write("control", "task_cancel", { id: selected.task_id });
        await refresh();
      } catch (error) {
        status.textContent = formatError(error);
      }
    };
    void loadTaskEvents({ reset: true });
  };
  const schedule = () => {
    clearTimeout(timer);
    if (!disposed && !document.hidden) timer = setTimeout(refresh, RECONCILE_MS);
  };
  const refresh = () => {
    if (disposed) return Promise.resolve();
    if (inFlight) {
      pending = true;
      return inFlight;
    }
    inFlight = rpc.read("control", "task_list")
      .then((tasks) => {
        if (!disposed) {
          renderTasks(tasks || []);
          status.textContent = "";
        }
      })
      .catch(() => {
        if (!disposed) status.textContent = "更新失败，将自动重试";
      })
      .finally(() => {
        inFlight = null;
        if (pending) {
          pending = false;
          void refresh();
        } else {
          schedule();
        }
      });
    return inFlight;
  };
  const visibility = () => {
    clearTimeout(timer);
    if (!document.hidden) void refresh();
  };
  const eventSubscription = events.subscribe("control.changed", (payload) => {
    const revision = Number(payload?.revision || 0);
    const domains = Array.isArray(payload?.domains) ? payload.domains : [];
    if (
      revision <= lastRevision ||
      !domains.some((domain) => domain === "tasks" || domain === "plugins")
    ) return;
    lastRevision = revision;
    if (document.hidden) return;
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => void refresh(), EVENT_DEBOUNCE_MS);
  }, "runtime.read");
  const connectionSubscription = events.onStateChange?.((state) => {
    if (state !== "open" || document.hidden) return;
    if (opened) void refresh();
    opened = true;
  });
  host.querySelector("[data-refresh]").onclick = () => void refresh();
  document.addEventListener("visibilitychange", visibility);
  void refresh();
  return {
    dispose() {
      disposed = true;
      clearTimeout(timer);
      clearTimeout(debounceTimer);
      eventSubscription.dispose();
      connectionSubscription?.dispose();
      document.removeEventListener("visibilitychange", visibility);
      traj?.destroy?.();
    },
  };
}

export default {
  id: "control",
  setup(ctx) {
    const pages = [
      ["control.plugins", "/plugins", "插件", 10, mountPlugins],
      ["control.runners", "/runners", "运行器", 20, mountRunners],
      ["control.events", "/events", "事件源", 30, mountEventSources],
      ["control.topology", "/topology", "拓扑", 40, mountTopology],
      ["control.tasks", "/tasks", "任务", 50, mountTasks],
      ["control.lifecycle", "/lifecycle", "生命周期", 70, mountLifecycle],
      ["control.logs", "/logs", "日志", 80, mountLogs],
    ];
    for (const [id, path, title, order, mount] of pages) {
      ctx.pages.register({
        id,
        path,
        title,
        component: { mount: (element) => mount(element, ctx.rpc, ctx.events) },
        requiredCapability: "runtime.read",
      });
      ctx.navigation.register({
        id: `${id}.nav`,
        activityId: "system",
        pageId: id,
        label: title,
        order,
        requiredCapability: "runtime.read",
      });
    }
  },
};
