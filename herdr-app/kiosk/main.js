// Kiosk, the repository and branch picker, in a terminal over the window.
//
// `mod.open:kiosk` opens this mod as a small dialog (the launcher). It starts
// kiosk with hc, then swaps itself for a large dialog holding kiosk's terminal
// under a bar, which is this mod again with props { terminal, notices }. The
// dialog closes when kiosk exits; the bar then shows kiosk's notifications.
//
// The binary is <checkout>/target/release/herdr-kiosk, where <checkout> is the
// repository this folder lives in: link this folder into the app's mods/ as
// `kiosk` (ln -s <checkout>/herdr-app/kiosk ~/.config/herdr-app/mods/kiosk).

import { View, div } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { appHome, closeOverlay, exec, focused, hc, home, notify, openOverlay, panes, props, readTextFile, spaces, theme } from "herdr";
import * as fs from "fs/promises";

const NAME = "kiosk";
const TITLE = "Kiosk";

// The herdr TUI's variables, which a core started from a herdr pane passes on.
// Kiosk refuses to start with them alongside the app's.
const TUI_VARIABLES = ["HERDR_BIN_PATH", "HERDR_PLUGIN_CONTEXT_JSON", "HERDR_PLUGIN_CONFIG_DIR", "HERDR_PLUGIN_STATE_DIR", "HERDR_PLUGIN_ROOT"];

// Keeps a failed kiosk on screen until the person has read why.
const WRAPPER = `"$1"; status=$?
if [ "$status" -ne 0 ]; then
  printf '\\nherdr-kiosk exited with status %s. Press enter to close.' "$status"
  read -r _
fi
exit "$status"`;

// A launch failure: what went wrong (a notification's title, which shows in full) and what to do.
class LaunchError extends Error {
  constructor(title, body) {
    super(`${title}: ${body}`);
    this.title = title;
    this.body = body;
  }
}

async function sh(script, ...args) {
  const r = await exec("/bin/sh", ["-c", script, "sh", ...args]);
  return { ok: r.code === 0, out: r.stdout.trim(), err: r.stderr.trim() };
}

// The repository checkout this mod folder is linked from.
async function checkout() {
  const dir = `${appHome()}/mods/${NAME}`;
  const r = await sh('cd -P "$1/../.." && pwd', dir);
  if (!r.ok || !r.out) throw new LaunchError("Kiosk's checkout is missing", `${dir}/../.. is not a folder: link mods/${NAME} from a herdr-kiosk checkout. ${r.err}`);
  return r.out;
}

async function program(name) {
  const r = await sh('command -v "$1"', name);
  if (!r.ok || !r.out.startsWith("/")) throw new LaunchError(`Kiosk can't find ${name}`, `${name} is not on the app's PATH.`);
  return r.out;
}

// Where a space is now: its first terminal's live folder (as the app names it).
function place(space) {
  if (!space) return null;
  const tabs = new Set(space.tabs.map((t) => t.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd ?? null;
}

async function launch() {
  const root = await checkout();
  const binary = `${root}/target/release/herdr-kiosk`;
  if (!(await sh('[ -x "$1" ]', binary)).ok) {
    throw new LaunchError("Kiosk isn't built", `Run cargo build --release in ${root} (${binary} is missing).`);
  }
  const [herdrApp, hcBinary] = await Promise.all([program("herdr-app"), program("hc")]);
  const now = focused();
  const space = spaces().find((s) => s.id === now?.space && !s.machine);
  const context = { focused_pane_cwd: space ? now.cwd : null, workspace_cwd: place(space) };
  const notices = `${appHome()}/data/${NAME}/notices-${Date.now()}-${Math.floor(Math.random() * 1e9)}.jsonl`;
  const argv = [
    "/usr/bin/env",
    ...TUI_VARIABLES.flatMap((v) => ["-u", v]),
    `HERDR_APP_HOME=${appHome()}`,
    `HERDR_KIOSK_APP_BIN=${herdrApp}`,
    `HERDR_KIOSK_HC_BIN=${hcBinary}`,
    `HERDR_KIOSK_NOTICE_FILE=${notices}`,
    `HERDR_KIOSK_CONTEXT_JSON=${JSON.stringify(context)}`,
    "/bin/sh", "-c", WRAPPER, "herdr-kiosk", binary,
  ];
  const cwd = context.focused_pane_cwd || context.workspace_cwd || home();
  const r = await hc(["run", "--bg", "--json", "--no-parent", "--rm", "--cwd", cwd, "--", ...argv]);
  const terminal = r.code === 0 ? JSON.parse(r.stdout).address : null;
  if (!terminal) throw new LaunchError("Kiosk didn't start", `hc run failed: ${(r.stderr || r.stdout).trim()}`);
  openOverlay({
    kind: "dialog",
    modal: true,
    width: 0.9,
    height: 0.9,
    layout: {
      type: "column",
      children: [
        { type: "block", size: { px: 30 }, content: { mod: NAME, props: { terminal, notices } } },
        { type: "block", content: { terminal } },
      ],
    },
  });
}

export default class Kiosk extends View {
  init(_props, cx) {
    // Only the launcher starts kiosk; `herdr-app check` loads the mod with no props.
    const { overlay, terminal } = props() ?? {};
    if (!overlay || terminal) return;
    cx.spawn(async (cx) => {
      try {
        await launch();
      } catch (error) {
        const known = error instanceof LaunchError;
        notify({ title: known ? error.title : "Kiosk didn't start", body: known ? error.body : String(error), level: "error" });
      }
      closeOverlay(props().overlay);
      cx.notify();
    });
  }

  async close() {
    const { terminal, notices } = props() ?? {};
    if (!terminal) return;
    // Notifications first: the close budget is short and they are what the person sees.
    const text = await readTextFile(notices);
    if (text !== null) {
      for (const line of text.split("\n").filter(Boolean)) {
        const { title, body } = JSON.parse(line);
        notify({ title, body, level: "warning" });
      }
      await fs.unlink(notices);
    }
    // Kiosk has normally exited by now (that closes the dialog); this ends it when the
    // dialog was closed some other way. hc's refusal for an ended terminal is expected.
    await hc(["kill", terminal]);
  }

  render() {
    const t = theme();
    if (!props()?.terminal) {
      return v_flex().size_full().p(16).gap(6).bg(t.popover).text_size(13)
        .child(div().font_semibold().text_color(t.foreground).child(TITLE))
        .child(div().text_color(t.muted).child("Starting…"));
    }
    return h_flex().size_full().px(12).gap(8).items_center().bg(t.surface).border_b(1).border_color(t.border).text_size(12)
      .child(div().font_semibold().text_color(t.foreground).child(TITLE))
      .child(div().flex_1().truncate().text_color(t.dim).child("repositories and branches"))
      .child(
        div().id("kiosk-close").px(8).py(2).rounded(5).text_color(t.muted).cursor_pointer()
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click(() => closeOverlay(props().overlay))
          .child("✕"),
      );
  }
}
