// Background jobs: processes agents started with `bash` in the background.
// A table of every job, and the selected job's log tail.

import { confirmDialog, displayPath, formatDuration, fullTime, h, plural, relativeTime, setChildren, toast } from "./dom.js";
import { icon } from "./icons.js";
import { displayName, jobOutput, killJob, openAgent, renderKeyed, state, tabById } from "./store.js";
import { statusDot } from "./widgets.js";

const TAIL_LINES = 400;
const POLL_MS = 2000;

let instances = 0;

export function runtimeOf(job) {
  const end = job.finished ? Date.parse(job.finished) : Date.now();
  return formatDuration(end - Date.parse(job.started));
}

export function jobStatus(job) {
  if (job.status === "running") return { dot: "running", text: "Running" };
  if (job.status === "killed") return { dot: "cancelled", text: "Killed" };
  if (job.status === "exited") return job.exit_code === 0 || job.exit_code == null ? { dot: "completed", text: "Exited" } : { dot: "failed", text: "Failed" };
  return { dot: "idle", text: "Unknown" };
}

export async function killJobFlow(job) {
  const ok = await confirmDialog({
    title: "Stop this job?",
    message: `${job.command} gets SIGTERM, then SIGKILL after 3 seconds.`,
    confirm: "Stop job",
    danger: true,
  });
  if (!ok) return;
  try {
    await killJob(job.id);
  } catch (error) {
    toast(error.message, "error");
  }
}

export function jobsView() {
  const slot = `jobs${++instances}`;
  const table = h("div", { class: "jobs-table-wrap" });
  const summary = h("p", { class: "page-summary" });
  const logHost = h("section", { class: "job-log", "aria-label": "Job output" });
  const root = h(
    "div",
    { class: "page jobs-page" },
    h("header", { class: "page-head" }, h("div", {}, h("h1", { class: "page-title" }, "Jobs"), summary)),
    h("div", { class: "jobs-layout" }, table, logHost),
  );
  let selected = null;
  let output = null;
  let timer = 0;
  let follow = true;
  // Runtimes of running jobs tick once a second.
  const tick = setInterval(() => {
    if ([...state.jobs.values()].some((j) => j.status === "running")) view.render();
  }, 1000);

  const sorted = () => [...state.jobs.values()].sort((a, b) => String(b.started).localeCompare(String(a.started)));

  const loadLog = async () => {
    clearTimeout(timer);
    const job = selected ? state.jobs.get(selected) : null;
    if (!job) return;
    try {
      output = await jobOutput(job.id, TAIL_LINES);
    } catch (error) {
      output = { error: error.message };
    }
    if (!root.isConnected) return;
    renderLog();
    if (state.jobs.get(selected)?.status === "running") timer = setTimeout(loadLog, POLL_MS);
  };

  const renderLog = () => {
    const job = selected ? state.jobs.get(selected) : null;
    if (!job) {
      setChildren(logHost, h("div", { class: "preview-empty" }, "Select a job to see its output."));
      return;
    }
    const status = jobStatus(job);
    const pre = h("pre", { class: "output job-output" }, output?.error ? output.error : output?.text || (output ? "(no output yet)" : "Loading…"));
    setChildren(
      logHost,
      h(
        "header",
        { class: "preview-head" },
        h("span", { class: "status" }, statusDot(status.dot), status.text),
        h("span", { class: "preview-title ellipsis", title: job.command }, job.command),
        h(
          "span",
          { class: "row-actions" },
          h("button", { class: "icon-button small", type: "button", "aria-label": "Refresh output", title: "Refresh", onclick: loadLog }, icon("refresh", 14)),
          job.status === "running" ? h("button", { class: "button small danger", type: "button", onclick: () => killJobFlow(job) }, icon("stop", 14), "Stop") : null,
        ),
      ),
      h(
        "dl",
        { class: "kv job-facts" },
        h("dt", {}, "Folder"),
        h("dd", { class: "path", title: job.cwd }, displayPath(job.cwd)),
        h("dt", {}, "Log"),
        h("dd", { class: "path", title: job.log }, displayPath(job.log)),
        output?.truncated ? [h("dt", {}, "Shown"), h("dd", {}, `Last ${TAIL_LINES} lines of ${Math.round((output.size ?? 0) / 1024)} KB`)] : null,
      ),
      pre,
    );
    if (follow) pre.scrollTop = pre.scrollHeight;
    pre.addEventListener("scroll", () => (follow = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 24));
  };

  const select = (id) => {
    selected = id;
    output = null;
    follow = true;
    view.render();
    renderLog();
    loadLog();
  };

  const view = {
    root,
    render() {
      const jobs = sorted();
      const running = jobs.filter((j) => j.status === "running").length;
      summary.textContent = jobs.length ? `${plural(jobs.length, "job")} · ${running} running` : "No background jobs";
      if (!selected && jobs.length) {
        selected = jobs[0].id;
        loadLog();
      }
      const key = JSON.stringify([selected, jobs.map((j) => [j.id, j.status, j.exit_code, j.finished]), Math.floor(Date.now() / 1000)]);
      renderKeyed(`${slot}:table`, key, () => setChildren(table, jobsTable(jobs, selected, select)));
      if (selected && !output) renderLog();
    },
    jobChanged() {
      if (selected && state.jobs.get(selected)?.status !== "running") loadLog();
    },
    dispose() {
      clearTimeout(timer);
      clearInterval(tick);
    },
  };
  return view;
}

function jobsTable(jobs, selected, select) {
  if (!jobs.length) {
    return h(
      "div",
      { class: "empty-state" },
      h("p", { class: "empty-title" }, "No background jobs"),
      h("p", { class: "muted" }, "When an agent starts a server or watcher in the background, it shows up here with its output."),
    );
  }
  return h(
    "table",
    { class: "table jobs-table" },
    h(
      "thead",
      {},
      h(
        "tr",
        {},
        h("th", {}, "Command"),
        h("th", {}, "Status"),
        h("th", { class: "col-agent" }, "Agent"),
        h("th", { class: "num col-age" }, "Started"),
        h("th", { class: "num" }, "Runtime"),
        h("th", { class: "num col-exit" }, "Exit"),
        h("th", { class: "col-actions" }, h("span", { class: "sr-only" }, "Actions")),
      ),
    ),
    h(
      "tbody",
      {},
      jobs.map((job) => {
        const status = jobStatus(job);
        const agent = job.agent ? state.agents.get(job.agent) : null;
        const tab = job.tab ? tabById(job.tab) : null;
        return h(
          "tr",
          { class: job.id === selected ? "selected" : "" },
          h(
            "td",
            { class: "cell-main" },
            h("button", { class: "row-link", type: "button", "aria-pressed": String(job.id === selected), title: job.command, onclick: () => select(job.id) }, job.command),
            h("div", { class: "agent-activity ellipsis", title: displayPath(job.cwd) }, tab ? tab.name : displayPath(job.cwd)),
          ),
          h("td", { class: "cell-status" }, h("span", { class: "status" }, statusDot(status.dot), status.text)),
          h(
            "td",
            { class: "col-agent cell-name" },
            agent ? h("button", { class: "row-link muted", type: "button", onclick: () => openAgent(agent.id) }, displayName(agent)) : h("span", { class: "muted" }, "–"),
          ),
          h("td", { class: "num muted col-age", title: fullTime(job.started) }, relativeTime(job.started)),
          h("td", { class: "num" }, runtimeOf(job)),
          h("td", { class: "num col-exit" }, job.exit_code ?? "–"),
          h(
            "td",
            { class: "col-actions" },
            job.status === "running"
              ? h("button", { class: "icon-button small", type: "button", "aria-label": `Stop ${job.command}`, title: "Stop job", onclick: () => killJobFlow(job) }, icon("stop", 14))
              : null,
          ),
        );
      }),
    ),
  );
}
