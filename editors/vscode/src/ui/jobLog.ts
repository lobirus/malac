// A job's log as a read-only document (`malac-job:` URIs), refreshed while
// the job runs and the document is open.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import type { DaemonStore } from "../model/store";

export const JOB_SCHEME = "malac-job";
const TAIL_LINES = 2000;
const POLL_MS = 2000;

export function jobUri(id: string, command: string): vscode.Uri {
  const name = command.split(/\s+/)[0]?.replace(/[^\w.-]+/g, "") || "job";
  return vscode.Uri.from({ scheme: JOB_SCHEME, path: `/${id}/${name}.log` });
}

export class JobLogProvider implements vscode.TextDocumentContentProvider, vscode.Disposable {
  private readonly changed = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.changed.event;
  private timer: ReturnType<typeof setInterval>;

  constructor(
    private readonly client: DaemonClient,
    private readonly store: DaemonStore,
  ) {
    this.timer = setInterval(() => this.poll(), POLL_MS);
  }

  async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
    const id = uri.path.split("/")[1];
    try {
      const reply = await this.client.request({ type: "job_output", job: id, tail: TAIL_LINES });
      const output = reply.output as { text: string; truncated: boolean; size: number; job: { command: string; status: string; exit_code: number | null } };
      const head = output.truncated ? `… (last ${TAIL_LINES} lines of ${output.size} bytes)\n` : "";
      const status = output.job.status === "running" ? "" : `\n[${output.job.status}${output.job.exit_code !== null ? ` ${output.job.exit_code}` : ""}]\n`;
      return `${head}${output.text}${status}`;
    } catch (error) {
      return `Cannot read job ${id}: ${error instanceof Error ? error.message : String(error)}`;
    }
  }

  /** Refreshes the open logs of running jobs, and a job's log once it ends. */
  refresh(id?: string): void {
    for (const document of vscode.workspace.textDocuments) {
      if (document.uri.scheme !== JOB_SCHEME) continue;
      const job = document.uri.path.split("/")[1];
      if (!id || job === id) this.changed.fire(document.uri);
    }
  }

  private poll(): void {
    for (const editor of vscode.window.visibleTextEditors) {
      const uri = editor.document.uri;
      if (uri.scheme !== JOB_SCHEME) continue;
      const job = this.store.jobs.get(uri.path.split("/")[1]);
      if (job?.status === "running") this.changed.fire(uri);
    }
  }

  dispose(): void {
    clearInterval(this.timer);
    this.changed.dispose();
  }
}
