// The Usage view: tokens and cost today, over the last seven days and per
// model, drawn as plain bars from `GET /api/usage`.

import { totalTokens, UsageBucket, UsageReport } from "../protocol";
import { formatCost, formatTokens, plural } from "./format";

export interface UsageRow {
  label: string;
  tokens: number;
  cost: number;
  sessions: number;
  /** 0..1, relative to the largest row of its group. */
  fraction: number;
  detail: string;
}

export interface UsageModel {
  summary: UsageRow[];
  models: UsageRow[];
  days: UsageRow[];
  hasCost: boolean;
}

function row(label: string, bucket: UsageBucket, max: number): UsageRow {
  const tokens = totalTokens(bucket);
  return {
    label,
    tokens,
    cost: bucket.cost ?? 0,
    sessions: bucket.sessions ?? 0,
    fraction: max > 0 ? Math.min(1, tokens / max) : 0,
    detail: [
      `Input ${formatTokens(bucket.input_tokens)}`,
      `output ${formatTokens(bucket.output_tokens)}`,
      `cache read ${formatTokens(bucket.cache_read_tokens)}`,
      `cache write ${formatTokens(bucket.cache_write_tokens)}`,
      plural(bucket.sessions ?? 0, "session"),
    ].join(", "),
  };
}

export function usageModel(report: UsageReport, today = new Date()): UsageModel {
  const week = totalTokens(report.week);
  const summary = [row("Today", report.today, week), row("Last 7 days", report.week, week)];
  const models = report.by_model.slice(0, 8);
  const modelMax = Math.max(0, ...models.map((m) => totalTokens(m)));
  const byDate = new Map(report.by_day.map((d) => [d.date, d]));
  const days: UsageRow[] = [];
  const dayMax = Math.max(0, ...report.by_day.map((d) => totalTokens(d)));
  for (let offset = 6; offset >= 0; offset -= 1) {
    const date = new Date(today.getFullYear(), today.getMonth(), today.getDate() - offset);
    const key = localDate(date);
    const bucket = byDate.get(key) ?? { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0, sessions: 0 };
    const label = offset === 0 ? "Today" : date.toLocaleDateString("en", { weekday: "short" });
    days.push(row(label, bucket, dayMax));
  }
  return {
    summary,
    models: models.map((m) => row(m.model, m, modelMax)),
    days,
    hasCost: report.week.cost > 0 || report.by_model.some((m) => m.cost > 0),
  };
}

export function localDate(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function escape(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Inline style attributes are blocked by the CSP, so sizes go into the nonced stylesheet. */
class Sizes {
  private rules: string[] = [];

  add(property: "width" | "height", value: string): string {
    const id = `s${this.rules.length}`;
    this.rules.push(`.${id}{${property}:${value}}`);
    return id;
  }

  css(): string {
    return this.rules.join("\n");
  }
}

function bar(r: UsageRow, hasCost: boolean, sizes: Sizes): string {
  const percent = Math.round(r.fraction * 100);
  const value = `${formatTokens(r.tokens)} tokens${hasCost && r.cost ? ` · ${formatCost(r.cost)}` : ""}`;
  const width = sizes.add("width", `${percent}%`);
  return `<div class="row" title="${escape(r.detail)}">
  <div class="row-head"><span class="row-label">${escape(r.label)}</span><span class="row-value">${escape(value)}</span></div>
  <div class="bar" role="meter" aria-label="${escape(`${r.label}: ${value}`)}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${percent}"><div class="fill ${width}"></div></div>
</div>`;
}

export const USAGE_CSS = `
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin: 0; padding: 8px 20px 16px 20px; font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); color: var(--vscode-foreground); background: transparent; }
h2 { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .04em; color: var(--vscode-descriptionForeground); margin: 14px 0 6px; }
h2:first-child { margin-top: 4px; }
.row { margin: 0 0 8px; }
.row-head { display: flex; justify-content: space-between; gap: 8px; margin-bottom: 3px; }
.row-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row-value { color: var(--vscode-descriptionForeground); font-variant-numeric: tabular-nums; white-space: nowrap; }
.bar { height: 4px; border-radius: 2px; background: var(--vscode-editorWidget-border, rgba(128,128,128,.25)); overflow: hidden; }
.fill { height: 100%; border-radius: 2px; background: var(--vscode-charts-green, #4ade80); opacity: .85; }
.days { display: grid; grid-template-columns: repeat(7, 1fr); gap: 4px; align-items: end; height: 56px; margin-top: 4px; }
.day { display: flex; flex-direction: column; align-items: center; gap: 3px; height: 100%; justify-content: flex-end; }
.col { width: 100%; max-width: 18px; min-height: 1px; border-radius: 2px 2px 0 0; background: var(--vscode-charts-green, #4ade80); opacity: .85; }
.day span { font-size: 10px; color: var(--vscode-descriptionForeground); }
.note, .empty { color: var(--vscode-descriptionForeground); font-size: 12px; line-height: 1.4; }
.note { margin-top: 12px; }
`;

/** The view's HTML; no scripts, so the CSP allows styles only. */
export function usageHtml(model: UsageModel | null, { nonce, message }: { nonce: string; message?: string }): string {
  const csp = `default-src 'none'; style-src 'nonce-${nonce}';`;
  const sizes = new Sizes();
  let body: string;
  if (!model) {
    body = `<p class="empty">${escape(message ?? "Connecting to malac…")}</p>`;
  } else {
    const days = model.days
      .map((d) => {
        const height = sizes.add("height", `${d.tokens ? Math.max(2, Math.round(d.fraction * 40)) : 1}px`);
        const title = `${d.label}: ${formatTokens(d.tokens)} tokens${d.cost ? `, ${formatCost(d.cost)}` : ""}`;
        return `<div class="day" title="${escape(title)}"><div class="col ${height}"></div><span>${escape(d.label.slice(0, 3))}</span></div>`;
      })
      .join("");
    body = [
      `<h2>Tokens</h2>`,
      model.summary.map((r) => bar(r, model.hasCost, sizes)).join(""),
      `<h2>Last 7 days</h2><div class="days" role="img" aria-label="Tokens per day over the last seven days">${days}</div>`,
      `<h2>By model, last 7 days</h2>`,
      model.models.length ? model.models.map((r) => bar(r, model.hasCost, sizes)).join("") : `<p class="empty">No usage recorded yet.</p>`,
      `<p class="note">From the session transcripts. A session counts on the day it was last active.</p>`,
    ].join("\n");
  }
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style nonce="${nonce}">${USAGE_CSS}\n${sizes.css()}</style></head><body>${body}</body></html>`;
}
