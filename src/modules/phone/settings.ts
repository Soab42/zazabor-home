import { listen } from "@tauri-apps/api/event";
import type { SettingsRow } from "../../appearance";
import type { Config } from "../../config";
import { open as openDialog } from "../../shared/dialog";
import type { Category } from "../../shell/settings";
import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { agoText, errorText, phoneApi, type BtState, type PhoneState, type RemoteInternetStatus, type RemoteSettings } from "./api";

/**
 * Settings → Phone: the link on or off, where files from the phone go, desktop notifications,
 * clipboard sharing, calls through this PC over Bluetooth, and the paired phones.
 */
export function phoneCategory(host: { readonly config: Config; configChanged(): void }): Category {
  const settings = () => host.config.phone;
  let state: PhoneState | null = null;

  const toggle = (label: string, hint: string, set: (on: boolean, input: HTMLInputElement) => void) => {
    const input = h("input", { type: "checkbox", class: "switch", role: "switch" });
    input.addEventListener("change", () => set(input.checked, input));
    return { input, row: { label, el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, label), h("p", { class: "sf-hint" }, hint)), input) } satisfies SettingsRow };
  };
  const problem = h("p", { class: "sf-hint pl-settings-error", hidden: true });
  const fail = (err: unknown) => {
    problem.hidden = false;
    problem.textContent = errorText(err);
  };

  // ---- the link
  const link = toggle("Phone Link", "Phones with Zazabor Link on the same Wi-Fi can find this PC and pair with it.", (on, input) => {
    settings().enabled = on;
    host.configChanged();
    void phoneApi.linkEnable(on).then((next) => {
      state = next;
      drawPhones();
    }, (err) => {
      input.checked = !on;
      settings().enabled = !on;
      fail(err);
    });
  });
  const pcName = h("span", { class: "st-value" }, "—");
  const pcRow: SettingsRow = { label: "This PC's name", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "This PC's name"), h("p", { class: "sf-hint" }, "What phones show when they look for a PC.")), pcName) };

  const folder = h("p", { class: "sf-hint pl-settings-path" });
  const choose = h("button", { class: "btn small", type: "button" }, "Choose…");
  const reset = h("button", { class: "btn small ghost", type: "button" }, "Default");
  choose.addEventListener("click", async () => {
    const dir = await openDialog({ title: "Files from the phone go to", directory: true, defaultPath: settings().downloadDir || undefined });
    if (!dir) return;
    settings().downloadDir = dir;
    host.configChanged();
    drawFolder();
  });
  reset.addEventListener("click", () => {
    settings().downloadDir = "";
    host.configChanged();
    drawFolder();
  });
  const folderRow: SettingsRow = { label: "Download folder", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Files from the phone go to"), folder), h("div", { class: "pl-settings-buttons" }, reset, choose)) };
  const drawFolder = () => {
    folder.textContent = settings().downloadDir || "Downloads › Zazabor Link";
    reset.hidden = !settings().downloadDir;
  };

  // ---- notifications and the clipboard
  const notify = toggle("Show new texts on the desktop", "A notification with who wrote, unless that conversation is open in Zazabor Home.", (on) => ((settings().notify = on), host.configChanged()));
  const mirrorNotifs = toggle("Show the phone's notifications on the desktop", "Notifications from apps on the phone show on this PC too.", (on) => ((settings().mirrorNotifs = on), host.configChanged()));
  const clip = toggle("Share the clipboard", "Text you copy on this PC can be pasted on the phone, and the other way round. Turn it on in Zazabor Link too.", (on, input) => {
    settings().clip = on;
    host.configChanged();
    const phones = state?.phones ?? [];
    void Promise.all(phones.map((p) => phoneApi.clipShare(p.phoneId, on))).catch((err) => {
      input.checked = !on;
      settings().clip = !on;
      fail(err);
    });
  });

  // ---- calls
  const btStatus = h("p", { class: "sf-hint pl-settings-bt" });
  const calls = toggle("Calls through this PC", "Make and take calls with this PC's speakers and microphone over Bluetooth. Zazabor Home becomes the phone's hands-free: it asks the sound server (WirePlumber) to leave that role to it and restarts it, which cuts sound for a moment. Headsets keep working.", (on, input) => {
    input.disabled = true;
    void phoneApi.btEnable(on).then((bt) => {
      settings().calls = on;
      host.configChanged();
      drawBt(bt);
    }, (err) => {
      input.checked = !on;
      fail(err);
    }).finally(() => (input.disabled = false));
  });
  calls.row.el.querySelector(".st-text")?.append(btStatus);
  const btSettings = h("button", { class: "btn small", type: "button" }, icon("bluetooth", 14), "Bluetooth Settings");
  btSettings.addEventListener("click", () => void phoneApi.btSettings().catch(fail));
  const btRow: SettingsRow = { label: "Pair a phone over Bluetooth", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Pair a phone over Bluetooth"), h("p", { class: "sf-hint" }, "The phone must be paired with this PC in the system's Bluetooth settings first.")), btSettings) };
  function drawBt(bt: BtState | null) {
    if (!bt) {
      btStatus.textContent = "Not available in this version of Zazabor Home.";
      calls.input.disabled = true;
      return;
    }
    calls.input.checked = bt.enabled;
    const on = bt.devices.find((d) => d.connected);
    btStatus.textContent = !bt.enabled ? "" : !bt.available ? "Bluetooth is off." : on ? `Connected: ${on.name}.` : bt.devices.some((d) => d.paired) ? "No phone is connected for calls." : "No phone is paired over Bluetooth.";
  }

  // ---- remote control: phones seeing and controlling this PC
  let remote: RemoteSettings = { enabled: false, always: [], viewOnly: false, encoder: "x264", internet: false, internetHost: "", media: false };
  const saveRemote = (patch: Partial<RemoteSettings>, undo: () => void) => {
    const next = { ...remote, ...patch };
    void phoneApi.remoteSetSettings(next).then((saved) => {
      remote = saved;
      drawRemote();
    }, (err) => {
      undo();
      fail(err);
    });
  };
  const remoteOn = toggle("Let paired phones see and control this PC", "A paired phone can ask to show this screen and use its mouse and keyboard. Every session asks here first (unless you chose “Always allow” for that phone), shows a red “Controlled from…” pill on every monitor, and Ctrl+Alt+Esc ends it.", (on, input) => saveRemote({ enabled: on }, () => (input.checked = !on)));
  const remoteView = toggle("View only", "Phones can watch the screen but not move the mouse or type.", (on, input) => saveRemote({ viewOnly: on }, () => (input.checked = !on)));
  const encoder = h("select", { "aria-label": "Video encoder" },
    h("option", { value: "x264" }, "Processor (x264, steady bitrate)"),
    h("option", { value: "vaapi" }, "Graphics chip (VA-API, constant quality)"));
  encoder.addEventListener("change", () => {
    const before = remote.encoder;
    saveRemote({ encoder: encoder.value as RemoteSettings["encoder"] }, () => (encoder.value = before));
  });
  const encoderRow: SettingsRow = { label: "Screen encoder", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Screen encoder"), h("p", { class: "sf-hint" }, "How the screen is compressed for the phone. The graphics chip uses less of the processor but can't hold a bitrate.")), encoder) };
  const remoteMedia = toggle("Let them see the camera and hear the microphone", "In a session the phone can switch the picture to this PC's webcam and listen to its microphone. A camera or microphone icon shows on the pill while they're on. (The PC's own sound can always be played on the phone.)", (on, input) => saveRemote({ media: on }, () => (input.checked = !on)));

  // Any network: the phone reaches this PC through link.<domain> on Meet's named tunnel.
  let internet: RemoteInternetStatus | null = null;
  const netInput = h("input", { type: "checkbox", class: "switch", role: "switch" });
  const netHint = h("p", { class: "sf-hint" });
  const hostInput = h("input", { class: "ops-input mtg-settings-input", type: "text", placeholder: "link.example.com", spellcheck: false, autocomplete: "off", autocapitalize: "off", "aria-label": "Hostname for phones" });
  const saveHost = h("button", { class: "btn small primary", type: "button" }, "Save");
  const testNet = h("button", { class: "btn small", type: "button" }, "Test");
  const netResult = h("p", { class: "sf-hint mtg-test", hidden: true });
  const tunnelState = h("p", { class: "sf-hint" });
  const busy = (button: HTMLButtonElement, work: Promise<unknown>) => {
    button.disabled = true;
    void work.finally(() => (button.disabled = false));
  };
  const saveNet = (patch: Partial<RemoteSettings>, undo: () => void) => {
    const next = { ...remote, ...patch };
    return phoneApi.remoteSetSettings(next).then((saved) => {
      remote = saved;
      drawRemote();
      void refreshInternet();
    }, (err) => {
      undo();
      fail(err);
    });
  };
  netInput.addEventListener("change", () => {
    const on = netInput.checked;
    if (on && !hostInput.value.trim()) {
      netInput.checked = false;
      fail("Type the hostname phones will use (like link.example.com) and press Save first.");
      return;
    }
    void saveNet({ internet: on, internetHost: hostInput.value.trim() || remote.internetHost }, () => (netInput.checked = !on));
  });
  saveHost.addEventListener("click", () => busy(saveHost, saveNet({ internetHost: hostInput.value.trim() }, () => undefined)));
  testNet.addEventListener("click", () => {
    netResult.hidden = false;
    netResult.dataset.result = "busy";
    netResult.textContent = "Testing… (the tunnel starts first if it's off)";
    busy(testNet, phoneApi.remoteInternetTest().then((r) => {
      netResult.dataset.result = r.ok ? "this-pc" : "unreachable";
      netResult.textContent = (r.ok ? "✓ " : "✗ ") + r.message;
    }, (err) => {
      netResult.dataset.result = "unreachable";
      netResult.textContent = `✗ ${errorText(err)}`;
    }));
  });
  const code = (text: string) => h("code", null, text);
  const step = (...parts: (Node | string)[]) => h("li", null, ...parts);
  const netGuide = h("details", { class: "mtg-guide" },
    h("summary", null, "How to set it up"),
    h("ol", null,
      step("It uses the Cloudflare tunnel of Meet's “My own domain”: set that up first (Settings → Meet)."),
      step("In the Cloudflare dashboard open that tunnel (Zero Trust → Networks → Tunnels) and add a second public hostname: subdomain ", code("link"), ", your domain, service type ", code("HTTP"), ", URL ", code("localhost:47214"), "."),
      step("Type the full hostname (for example ", code("link.example.com"), ") above, press Save, switch this on, then Test."),
      step("Open Zazabor Link once while the phone is on the same Wi-Fi as this PC: it learns the address then, and uses it whenever it can't find the PC on Wi-Fi."),
    ),
    h("p", { class: "sf-hint" }, "Everything stays end-to-end encrypted with the phone's own pairing key: Cloudflare only passes sealed bytes along. Pairing a new phone still needs the same Wi-Fi. Mirroring the phone's screen on this PC works on the same Wi-Fi only, for now."),
  );
  const netRow: SettingsRow = {
    label: "Over the internet",
    el: h("div", { class: "sf-row wide" },
      h("label", { class: "sf-row sf-toggle pl-net-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Reach this PC from any network"), netHint), netInput),
      h("span", { class: "sf-label mtg-gap" }, "Hostname for phones"), hostInput,
      h("div", { class: "pl-settings-buttons mtg-gap" }, saveHost, testNet), netResult, tunnelState,
      netGuide),
  };
  // The tunnel's state changes on its own (connecting, dropping): follow it.
  void listen("remote-changed", () => void refreshInternet()).catch(() => undefined);
  async function refreshInternet() {
    internet = await phoneApi.remoteInternetStatus().catch(() => null);
    drawInternet();
  }
  function drawInternet() {
    netInput.checked = remote.internet;
    if (document.activeElement !== hostInput) hostInput.value = remote.internetHost || (internet?.meetHost ? internet.meetHost.replace(/^[^.]+\./, "link.") : "");
    netHint.textContent = !internet?.hasToken
      ? "Needs Meet's “My own domain” tunnel token first (Settings → Meet)."
      : "Texts, notifications, files and Remote work away from home, over mobile data or any Wi-Fi.";
    const t = internet?.tunnel;
    tunnelState.textContent = !remote.internet || !t ? "" : internet?.problem ? `Not running: ${internet.problem}` : t.state === "on" ? `Tunnel connected. Phones dial ${remote.internetHost}.` : t.state === "starting" ? "Tunnel starting…" : t.state === "error" ? `Tunnel problem: ${t.error ?? "unknown"}` : "Tunnel off.";
  }

  const alwaysBox = h("div", { class: "pl-settings-phones" });
  const alwaysRow: SettingsRow = { label: "Always allowed", el: alwaysBox };
  function drawRemote() {
    remoteOn.input.checked = remote.enabled;
    remoteView.input.checked = remote.viewOnly;
    remoteMedia.input.checked = remote.media;
    encoder.value = remote.encoder;
    drawInternet();
    const names = new Map((state?.phones ?? []).map((p) => [p.phoneId, p.name]));
    if (!remote.always.length) {
      alwaysBox.replaceChildren(h("p", { class: "sf-hint pl-settings-none" }, "No phone is always allowed: each session asks first."));
      return;
    }
    alwaysBox.replaceChildren(...remote.always.map((id) => {
      const revoke = h("button", { class: "btn small ghost", type: "button" }, "Ask every time");
      revoke.addEventListener("click", () => saveRemote({ always: remote.always.filter((p) => p !== id) }, () => undefined));
      return h("div", { class: "sf-row plain pl-settings-phone" },
        h("span", { class: "pl-card-device small" }, icon("phone", 16)),
        h("div", { class: "st-text" }, h("span", { class: "sf-label" }, names.get(id) ?? "A phone that is no longer paired"), h("p", { class: "sf-hint" }, "Can start a session without asking.")),
        revoke);
    }));
  }

  // ---- paired phones
  const phonesBox = h("div", { class: "pl-settings-phones" });
  const phonesRow: SettingsRow = { label: "Paired phones", el: phonesBox };
  function drawPhones() {
    drawRemote();
    if (state) {
      pcName.textContent = state.pcName || "—";
      link.input.checked = state.enabled;
      clip.input.checked = state.clip || settings().clip;
    }
    const phones = state?.phones ?? [];
    if (!phones.length) {
      phonesBox.replaceChildren(h("p", { class: "sf-hint pl-settings-none" }, state ? "No phone is paired yet. Open Zazabor Link on the phone and pick this PC." : "Phone Link isn't available in this version of Zazabor Home."));
      return;
    }
    phonesBox.replaceChildren(...phones.map((p) => {
      const forget = h("button", { class: "btn small ghost", type: "button" }, "Forget");
      forget.addEventListener("click", () => {
        forget.disabled = true;
        void phoneApi.forget(p.phoneId).then(async () => {
          state = await phoneApi.state();
          drawPhones();
        }, (err) => {
          forget.disabled = false;
          fail(err);
        });
      });
      return h("div", { class: "sf-row plain pl-settings-phone" },
        h("span", { class: `pl-card-device small${p.connected ? " on" : ""}` }, icon("phone", 16)),
        h("div", { class: "st-text" }, h("span", { class: "sf-label" }, p.name), h("p", { class: "sf-hint" }, `${p.model} · ${p.connected ? (p.internet ? "Connected over the internet" : "Connected") : `Last seen ${agoText(p.lastSeen)}`}`)),
        forget);
    }));
  }

  return {
    id: "phone", title: "Phone", icon: "phone", section: "Modules",
    intro: problem,
    groups: [
      { title: "Phone Link", rows: [link.row, pcRow, folderRow] },
      { title: "Notifications", rows: [notify.row, mirrorNotifs.row, clip.row] },
      { title: "Calls", rows: [calls.row, btRow] },
      { title: "Remote control", rows: [remoteOn.row, remoteView.row, remoteMedia.row, encoderRow, netRow, alwaysRow] },
      { title: "Paired phones", rows: [phonesRow] },
    ],
    refresh() {
      problem.hidden = true;
      link.input.checked = settings().enabled;
      notify.input.checked = settings().notify;
      mirrorNotifs.input.checked = settings().mirrorNotifs;
      clip.input.checked = settings().clip;
      calls.input.checked = settings().calls;
      drawFolder();
      void phoneApi.state().then((next) => (state = next), () => (state = null)).then(drawPhones);
      void phoneApi.btState().then(drawBt, () => drawBt(null));
      void phoneApi.remoteSettings().then((next) => (remote = next), () => undefined).then(drawRemote).then(refreshInternet);
    },
  };
}
