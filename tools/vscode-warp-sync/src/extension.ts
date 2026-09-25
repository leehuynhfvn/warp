import * as path from "node:path";
import * as vscode from "vscode";
import { planCompare } from "./compare";
import { changedFiles } from "./git";
import { isMirrorCandidate } from "./mirror";
import { overwritePrompt, Prompt, uploadPrompt, count, formatSize } from "./prompts";
import { ComparedResult, SyncResult } from "./protocol";
import { runProcess } from "./runner";
import { describeFailure, describeStatus, StatusView } from "./status";
import { WarpctrlClient, WarpctrlError } from "./warpctrl";

const CONTEXT_ACTIVE = "warpSync.active";
const STATUS_POLL_MS = 30_000;
const DEFAULT_COMMAND = ["warpctrl"];

function settings(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration("warpSync");
}

function configuredCommand(): readonly string[] {
  const command = settings().get<unknown>("command");
  const valid =
    Array.isArray(command) &&
    command.length > 0 &&
    command.every((part) => typeof part === "string" && part !== "");
  return valid ? (command as string[]) : DEFAULT_COMMAND;
}

class Controller implements vscode.Disposable {
  private readonly statusItem: vscode.StatusBarItem;
  private readonly timer: NodeJS.Timeout;
  private mirrorFolder: string | undefined;

  constructor(
    private readonly client: WarpctrlClient,
    private readonly output: vscode.OutputChannel,
  ) {
    this.statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
    this.statusItem.command = "warpSync.status";
    this.statusItem.name = "Warp Sync";
    this.timer = setInterval(() => {
      if (vscode.window.state.focused && this.mirrorFolder !== undefined) {
        void this.refresh();
      }
    }, STATUS_POLL_MS);
  }

  dispose(): void {
    clearInterval(this.timer);
    this.statusItem.dispose();
  }

  private candidateFolders(): string[] {
    const root = settings().get<string>("mirrorRoot", "");
    return (vscode.workspace.workspaceFolders ?? [])
      .map((folder) => folder.uri.fsPath)
      .filter((folder) => isMirrorCandidate(folder, root));
  }

  private async setActive(folder: string | undefined): Promise<void> {
    this.mirrorFolder = folder;
    await vscode.commands.executeCommand("setContext", CONTEXT_ACTIVE, folder !== undefined);
    if (folder === undefined) {
      this.statusItem.hide();
    }
  }

  private show(view: StatusView): void {
    this.statusItem.text = view.text;
    this.statusItem.tooltip = view.tooltip;
    this.statusItem.backgroundColor = view.warning
      ? new vscode.ThemeColor("statusBarItem.warningBackground")
      : undefined;
    this.statusItem.show();
  }

  /** Asks Warp which sessions serve the mirror folder and updates the status bar. */
  async refresh(): Promise<StatusView | undefined> {
    const folder = this.candidateFolders()[0];
    if (folder === undefined) {
      await this.setActive(undefined);
      return undefined;
    }
    try {
      const result = await this.client.status(folder);
      if (result.status !== "status" || result.path === undefined) {
        await this.setActive(undefined);
        return undefined;
      }
      await this.setActive(folder);
      const view = describeStatus(result.path);
      this.show(view);
      return view;
    } catch (error) {
      if (error instanceof WarpctrlError && error.code === "invalid_params") {
        await this.setActive(undefined);
        return undefined;
      }
      await this.setActive(folder);
      const view = describeFailure(this.asWarpctrlError(error));
      this.show(view);
      return view;
    }
  }

  private asWarpctrlError(error: unknown): WarpctrlError {
    return error instanceof WarpctrlError
      ? error
      : new WarpctrlError(error instanceof Error ? error.message : String(error), "protocol");
  }

  async showStatus(): Promise<void> {
    const view = await this.refresh();
    if (view === undefined) {
      void vscode.window.showInformationMessage("Warp Sync: this workspace is not a Warp Sync mirror.");
      return;
    }
    const message = view.tooltip.replace(/\nClick to check again\.$/, "").replace(/\n/g, " · ");
    if (view.warning) {
      void vscode.window.showWarningMessage(message);
    } else {
      void vscode.window.showInformationMessage(message);
    }
  }

  private progress<T>(title: string, task: () => Promise<T>): Thenable<T> {
    return vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title },
      task,
    );
  }

  /** Runs `task`, reporting a failure to the user. Returns whether it went through. */
  private async guard(task: () => Promise<boolean>): Promise<boolean> {
    try {
      return await task();
    } catch (error) {
      const failure = this.asWarpctrlError(error);
      this.output.appendLine(`[${failure.kind}${failure.code ? `:${failure.code}` : ""}] ${failure.message}`);
      void vscode.window.showErrorMessage(`Warp Sync: ${failure.message}`);
      void this.refresh();
      return false;
    }
  }

  private async forEach(uris: readonly vscode.Uri[], operation: (uri: vscode.Uri) => Promise<boolean>): Promise<void> {
    for (const uri of uris) {
      if (!(await this.guard(() => operation(uri)))) {
        return;
      }
    }
  }

  private async refreshGit(): Promise<void> {
    try {
      await vscode.commands.executeCommand("git.refresh");
    } catch (error) {
      this.output.appendLine(`git.refresh failed: ${String(error)}`);
    }
  }

  /** Asks the user; answers Warp's pending question accordingly. Returns whether to go ahead. */
  private async ask(prompt: Prompt, pendingId: string): Promise<boolean> {
    const choice = await vscode.window.showWarningMessage(
      prompt.title,
      { modal: true, detail: prompt.detail },
      prompt.confirmLabel,
    );
    if (choice === prompt.confirmLabel) {
      return true;
    }
    try {
      await this.client.cancel(pendingId);
    } catch (error) {
      this.output.appendLine(`Could not cancel ${pendingId}: ${String(error)}`);
    }
    return false;
  }

  download(uris: readonly vscode.Uri[]): Promise<void> {
    return this.forEach(uris, async (uri) => {
      const name = path.basename(uri.fsPath);
      let result: SyncResult = await this.progress(`Warp Sync: downloading ${name}…`, () =>
        this.client.download(uri.fsPath),
      );
      if (result.status === "needs_confirmation" && result.kind === "overwrite_local_changes") {
        const pendingId = result.pending_id;
        if (!(await this.ask(overwritePrompt(result.files), pendingId))) {
          return false;
        }
        result = await this.progress(`Warp Sync: downloading ${name}…`, () =>
          this.client.confirm(pendingId),
        );
      }
      if (result.status !== "downloaded") {
        throw new WarpctrlError("Warp did not report the download.", "protocol");
      }
      const skipped = result.skipped.length === 0 ? "" : `, skipped ${result.skipped.length}`;
      void vscode.window.showInformationMessage(
        `Warp Sync: downloaded ${count(result.files, "file")} (${formatSize(result.bytes)}) as ${result.remote_user}${skipped}.`,
      );
      await this.refreshGit();
      return true;
    });
  }

  upload(uris: readonly vscode.Uri[]): Promise<void> {
    return this.forEach(uris, (uri) => this.uploadOne(uri.fsPath));
  }

  private async uploadOne(fsPath: string): Promise<boolean> {
    const name = path.basename(fsPath);
    const prepared = await this.progress(`Warp Sync: preparing upload of ${name}…`, () =>
      this.client.prepareUpload(fsPath),
    );
    if (prepared.status !== "needs_confirmation" || prepared.kind !== "upload") {
      throw new WarpctrlError("Warp did not ask to confirm the upload.", "protocol");
    }
    const pendingId = prepared.pending_id;
    if (!(await this.ask(uploadPrompt(prepared.summary), pendingId))) {
      return false;
    }
    const done = await this.progress(`Warp Sync: uploading ${name}…`, () =>
      this.client.confirm(pendingId),
    );
    if (done.status !== "uploaded") {
      throw new WarpctrlError("Warp did not report the upload.", "protocol");
    }
    const backup = done.backup_path === undefined ? "" : ` Previous version saved to ${done.backup_path}.`;
    void vscode.window.showInformationMessage(
      `Warp Sync: uploaded ${count(done.files, "file")} (${formatSize(done.bytes)}) as ${done.remote_user}.${backup}`,
    );
    await this.refreshGit();
    return true;
  }

  compare(uris: readonly vscode.Uri[]): Promise<void> {
    return this.forEach(uris, (uri) => this.compareOne(uri.fsPath));
  }

  private async compareOne(fsPath: string): Promise<boolean> {
    const name = path.basename(fsPath);
    const result = await this.progress(`Warp Sync: comparing ${name} with the server…`, () =>
      this.client.compare(fsPath),
    );
    if (result.status === "unchanged") {
      void vscode.window.showInformationMessage(
        `Warp Sync: no differences (${count(result.identical_files, "file")} match the server).`,
      );
      return true;
    }
    if (result.status !== "compared") {
      throw new WarpctrlError("Warp did not report the comparison.", "protocol");
    }
    await this.openComparison(result);
    return true;
  }

  private async openComparison(result: ComparedResult): Promise<void> {
    const plan = planCompare(result);
    if (plan.report !== undefined) {
      await vscode.window.showTextDocument(vscode.Uri.file(plan.report), { preview: true });
    }
    for (const diff of plan.diffs) {
      await vscode.commands.executeCommand(
        "vscode.diff",
        vscode.Uri.file(diff.left),
        vscode.Uri.file(diff.right),
        diff.title,
        { preview: false },
      );
    }
  }

  /** Lets the user pick among the files that changed in the mirror, then applies `operation`. */
  private async pickChanged(title: string): Promise<vscode.Uri[]> {
    const folder = this.mirrorFolder;
    if (folder === undefined) {
      return [];
    }
    const changed = await changedFiles(folder);
    if (changed === undefined) {
      void vscode.window.showWarningMessage(
        "Warp Sync: the mirror has no Git repository yet. Download a path once to create it.",
      );
      return [];
    }
    if (changed.length === 0) {
      void vscode.window.showInformationMessage("Warp Sync: no files have changed in the mirror.");
      return [];
    }
    const picked = await vscode.window.showQuickPick(
      changed.map((file) => ({ label: path.relative(folder, file), file, picked: true })),
      { canPickMany: true, title, placeHolder: "Choose the files" },
    );
    return (picked ?? []).map((item) => vscode.Uri.file(item.file));
  }

  async uploadChanged(): Promise<void> {
    await this.upload(await this.pickChanged("Warp Sync: upload changed files"));
  }

  async compareChanged(): Promise<void> {
    await this.compare(await this.pickChanged("Warp Sync: compare changed files"));
  }
}

/** The files a command applies to: the selection, the clicked resource, or the active editor. */
function targets(clicked?: vscode.Uri, selected?: vscode.Uri[]): vscode.Uri[] {
  if (selected !== undefined && selected.length > 0) {
    return selected;
  }
  if (clicked instanceof vscode.Uri) {
    return [clicked];
  }
  const active = vscode.window.activeTextEditor?.document.uri;
  return active !== undefined && active.scheme === "file" ? [active] : [];
}

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("Warp Sync");
  const client = new WarpctrlClient(runProcess, configuredCommand);
  const controller = new Controller(client, output);
  const register = (command: string, handler: (...args: never[]) => unknown): vscode.Disposable =>
    vscode.commands.registerCommand(command, handler);

  context.subscriptions.push(
    output,
    controller,
    register("warpSync.download", (clicked?: vscode.Uri, selected?: vscode.Uri[]) =>
      controller.download(targets(clicked, selected)),
    ),
    register("warpSync.upload", (clicked?: vscode.Uri, selected?: vscode.Uri[]) =>
      controller.upload(targets(clicked, selected)),
    ),
    register("warpSync.compare", (clicked?: vscode.Uri, selected?: vscode.Uri[]) =>
      controller.compare(targets(clicked, selected)),
    ),
    register("warpSync.uploadChanged", () => controller.uploadChanged()),
    register("warpSync.compareChanged", () => controller.compareChanged()),
    register("warpSync.status", () => controller.showStatus()),
    vscode.workspace.onDidChangeWorkspaceFolders(() => void controller.refresh()),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("warpSync")) {
        void controller.refresh();
      }
    }),
  );
  void controller.refresh();
}

export function deactivate(): void {}
