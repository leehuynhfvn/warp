// The changed files of the mirror's Git repository, from VS Code's built-in Git extension. The
// mirror is a Git repository whose HEAD is the state of the server at the last sync, so its
// working tree changes are exactly what has not been uploaded.
import * as vscode from "vscode";
import { GitChange, uploadableChanges } from "./changes";

interface GitChangeEntry {
  uri: vscode.Uri;
  status: number;
}

interface GitRepository {
  rootUri: vscode.Uri;
  state: {
    workingTreeChanges: GitChangeEntry[];
    indexChanges: GitChangeEntry[];
    mergeChanges: GitChangeEntry[];
  };
}

interface GitApi {
  repositories: GitRepository[];
}

interface GitExtension {
  getAPI(version: 1): GitApi;
}

async function gitApi(): Promise<GitApi | undefined> {
  const extension = vscode.extensions.getExtension<GitExtension>("vscode.git");
  if (extension === undefined) {
    return undefined;
  }
  const exports = extension.isActive ? extension.exports : await extension.activate();
  return exports.getAPI(1);
}

/** The files with changes in the repository at `folder`, or undefined when there is none. */
export async function changedFiles(folder: string): Promise<string[] | undefined> {
  const api = await gitApi();
  const repository = api?.repositories.find((candidate) => candidate.rootUri.fsPath === folder);
  if (repository === undefined) {
    return undefined;
  }
  const { workingTreeChanges, indexChanges, mergeChanges } = repository.state;
  const changes: GitChange[] = [...workingTreeChanges, ...indexChanges, ...mergeChanges].map(
    (change) => ({ path: change.uri.fsPath, status: change.status }),
  );
  return uploadableChanges(changes);
}
