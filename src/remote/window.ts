import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyAppearance } from "../appearance";
import { adoptConfig, CONFIG_SAVED, parseConfig, type Config } from "../config";
import { onOthers } from "../shell/windows";
import { h } from "../ui/dom";
import { icon } from "../ui/icons";
import "./remote.css";

/** What the backend says the window shows (`src-tauri/src/remote/ui.rs`). */
interface UiState {
  mode: "idle" | "ask" | "live";
  phone: string;
  viewOnly: boolean;
}

/**
 * The little windows of a remote-control session, one per monitor: the question "<phone> wants to
 * control this PC" (Allow once / Always allow / Deny), and while it runs a bright pill with a Stop
 * button. They only draw; the answers and Stop go to the backend, which owns the session.
 */
export async function startRemoteWindow(config: Config) {
  document.body.classList.add("remote-window");
  applyAppearance(config.appearance);
  void onOthers<string>(CONFIG_SAVED, (json) => {
    try {
      adoptConfig(config, parseConfig(json));
    } catch {
      return;
    }
    applyAppearance(config.appearance);
  });
  const root = h("div", { class: "rc-root" });
  document.getElementById("app")!.append(root);

  const answer = (choice: "once" | "always" | "deny") => void invoke("remote_answer", { choice }).catch(() => undefined);

  const render = (state: UiState) => {
    root.replaceChildren();
    root.dataset.mode = state.mode;
    if (state.mode === "ask") {
      const once = h("button", { class: "rc-btn primary", type: "button", onclick: () => answer("once") }, "Allow once");
      root.append(
        h("div", { class: "rc-card", role: "alertdialog", "aria-label": "Remote control request" },
          h("div", { class: "rc-head" }, h("span", { class: "rc-tile" }, icon("phone", 20)),
            h("div", { class: "rc-text" },
              h("div", { class: "rc-title" }, `${state.phone} wants to control this PC`),
              h("div", { class: "rc-sub" }, "It will see this screen and can move the mouse and type. Say no if you didn't expect this."))),
          h("div", { class: "rc-actions" }, once,
            h("button", { class: "rc-btn", type: "button", onclick: () => answer("always") }, "Always allow this phone"),
            h("button", { class: "rc-btn danger", type: "button", onclick: () => answer("deny") }, "Deny"))),
      );
      once.focus();
    } else if (state.mode === "live") {
      // A small badge in the corner (the user asked for it out of the way), still on every monitor.
      const who = `${state.viewOnly ? "Watched" : "Controlled"} from ${state.phone}. Ctrl+Alt+Esc or Stop ends it.`;
      root.append(
        h("div", { class: "rc-mini", role: "status", title: who, "aria-label": who },
          h("span", { class: "rc-badge" }, icon("phone", 16), h("span", { class: "rc-dot", "aria-hidden": "true" })),
          h("button", { class: "rc-stop", type: "button", title: "Stop remote control", "aria-label": "Stop remote control", onclick: () => void invoke("remote_stop").catch(() => undefined) }, icon("stop", 13))),
      );
    }
  };

  void listen<UiState>("remote-ui-state", (e) => render(e.payload));
  render(await invoke<UiState>("remote_ui_state").catch(() => ({ mode: "idle", phone: "", viewOnly: false }) as UiState));
}
