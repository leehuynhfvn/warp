// What the status bar says about the connection between the mirror and a Warp session.
import { PathStatus } from "./protocol";
import { WarpctrlError } from "./warpctrl";

export interface StatusView {
  text: string;
  tooltip: string;
  warning: boolean;
}

export function describeStatus(status: PathStatus | undefined): StatusView {
  if (status === undefined || status.sessions.length === 0) {
    const host = status === undefined ? "the server" : status.host_key;
    return {
      text: "$(warning) no Warp session",
      tooltip: `Warp Sync: open a session to ${host} in Warp to sync this mirror. Click to check again.`,
      warning: true,
    };
  }
  const chosen = status.sessions.find((session) => session.is_active) ?? status.sessions[0];
  const others = status.sessions.length - 1;
  const more = others > 0 ? ` (+${others})` : "";
  const lines = status.sessions.map(
    (session) =>
      `${session.user}@${session.hostname} — tab ${session.tab_index + 1}${session.is_active ? " (active)" : ""}`,
  );
  return {
    text: `$(cloud) ${chosen.user}@${chosen.hostname}${more}`,
    tooltip: `Warp Sync: commands run through\n${lines.join("\n")}\nClick to check again.`,
    warning: false,
  };
}

export function describeFailure(error: WarpctrlError): StatusView {
  return {
    text: "$(warning) Warp Sync",
    tooltip: `Warp Sync: ${error.message}\nClick to check again.`,
    warning: true,
  };
}
