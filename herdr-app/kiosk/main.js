// Kiosk, the repository and branch picker, drawn natively.
//
// `mod.open:kiosk` opens this mod as a popup. It runs `herdr-kiosk app`, which starts the
// picker in the background and prints where to reach it: a loopback port and a secret. The
// mod connects, and from then on the picker (Rust, the same state machine as kiosk's
// terminal UI) sends what to show and the mod sends what the person does. The popup closes
// when the picker exits; closing the popup some other way tells the picker to quit.
//
// The binary is <mod folder>/bin/herdr-kiosk, where the install script puts it, or, when
// this folder is linked from a herdr-kiosk checkout, that checkout's
// target/release/herdr-kiosk.

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { appHome, closeOverlay, exec, focused, notify, panes, props, spaces, theme, window } from "herdr";
import { Buffer } from "buffer";
import net from "net";

const NAME = "kiosk";
const TITLE = "Kiosk";
// The protocol this mod speaks; the picker refuses a different one.
const PROTOCOL = 1;

// The popup sizes to what it holds, so each list has a fixed number of rows the picker fills.
// One set of measurements for the whole picker, so edges and text line up everywhere:
// content sits EDGE from the popup's sides, a field's text INSET inside its border, and a
// row's highlight spans exactly the field's width, with its icon and text where the
// field's are.
const EDGE = 16;
const CARD_EDGE = 20;
const INSET = 11;
const BORDER = 1;
const ROW_INSET = INSET + BORDER;
const ICON = 15;
const ICON_GAP = 9;
const ROW = 32;
const HELP_ROW = 24;
const WARNING_ROW = 44;
const VIEWPORT = { list: 12, bases: 8, help: 14, scan_warnings: 7 };
const ICONS = "mods/workspace-chrome";
const CONTEXT = "KioskPicker";
// The most a paste sends, well inside the app's 1 MiB limit on one socket write.
const MAX_PASTE = 128 * 1024;

// A failure to tell the person about: a notification's title, and what to do about it.
class KioskError extends Error {
  constructor(title, body) {
    super(`${title}: ${body}`);
    this.title = title;
    this.body = body;
  }
}

// Where the binary may be: the install script's copy, then a linked checkout's build.
function binaries() {
  const dir = `${appHome()}/mods/${NAME}`;
  const exe = window().platform === "windows" ? ".exe" : "";
  return [`${dir}/bin/herdr-kiosk${exe}`, `${dir}/../../target/release/herdr-kiosk${exe}`];
}

// Where a space is now: its first terminal's live folder (as the app names it).
function place(space) {
  if (!space) return null;
  const tabs = new Set(space.tabs.map((t) => t.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd ?? null;
}

function context() {
  const now = focused();
  const space = spaces().find((s) => s.id === now?.space && !s.machine);
  return { focused_pane_cwd: space ? now.cwd : null, workspace_cwd: place(space) };
}

// Runs `herdr-kiosk app` from the first place it can, returning where the picker listens.
async function launch() {
  const args = ["app", "--app-home", appHome(), "--context", JSON.stringify(context())];
  const missing = [];
  for (const program of binaries()) {
    let r;
    try {
      r = await exec(program, args);
    } catch (error) {
      missing.push(`${program} (${error})`);
      continue;
    }
    if (r.code !== 0) throw new KioskError("Kiosk didn't start", (r.stderr || r.stdout).trim() || `${program} exited with ${r.code}`);
    try {
      return { program, handshake: JSON.parse(r.stdout.trim()) };
    } catch {
      throw new KioskError("Kiosk didn't start", `${program} printed no handshake: ${r.stdout.trim()}`);
    }
  }
  throw new KioskError("Kiosk isn't installed", `Install it with its install script, or build a linked checkout with cargo build --release. Tried ${missing.join("; ")}.`);
}

async function start() {
  const { program, handshake } = await launch();
  if (handshake.protocol !== PROTOCOL) {
    throw new KioskError("Kiosk needs updating", `This mod speaks protocol ${PROTOCOL} and ${program} speaks ${handshake.protocol}. Reinstall kiosk so the mod and binary match.`);
  }
  const socket = await net.connect("127.0.0.1", handshake.port);
  return { socket, token: handshake.token };
}

// Lines of a socket's bytes, decoded once whole so a character split across reads survives.
class Lines {
  constructor() {
    this.pending = new Uint8Array(0);
  }

  push(chunk) {
    const joined = new Uint8Array(this.pending.length + chunk.length);
    joined.set(this.pending);
    joined.set(chunk, this.pending.length);
    const lines = [];
    let start = 0;
    for (let i = 0; i < joined.length; i++) {
      if (joined[i] === 10) {
        lines.push(Buffer.from(joined.subarray(start, i)).toString("utf8"));
        start = i + 1;
      }
    }
    this.pending = joined.slice(start);
    return lines;
  }
}

// What each of the picker's tones looks like in the app's theme.
function toneColor(t, tone) {
  switch (tone) {
    case "muted":
    case "neutral":
      return t.muted;
    case "faint":
    case "hint":
      return t.dim;
    case "accent":
    case "secondary":
    case "tertiary":
      return t.accent;
    case "warning":
    case "attention":
      return t.blocked;
    case "error":
    case "failure":
      return t.ansi1;
    case "open":
    case "success":
      return t.done;
    case "progress":
      return t.working;
    default:
      return t.foreground;
  }
}

const icon = (t, name, color, size = 14) => svg(`${ICONS}/${name}.svg`).size(size).flex_none().text_color(color ?? t.dim);

function spinner(t, color) {
  return svg(`${ICONS}/spinner.svg`).size(12).flex_none().text_color(color ?? t.dim).spin(900);
}

// A key as the app writes keys: symbols on macOS (⌃H), words elsewhere (Ctrl+H).
function keyLabel(key) {
  const mac = window().platform === "macos";
  const names = mac
    ? { "ctrl+": "⌃", "alt+": "⌥", "shift+": "⇧", enter: "↩", tab: "⇥", backspace: "⌫", esc: "esc" }
    : { "ctrl+": "Ctrl+", "alt+": "Alt+", "shift+": "Shift+", enter: "Enter", tab: "Tab", backspace: "Backspace", esc: "Esc" };
  return key.split("/").map((part) => {
    let rest = part;
    let out = "";
    for (const modifier of ["ctrl+", "alt+", "shift+"]) {
      if (rest.startsWith(modifier)) {
        out += names[modifier];
        rest = rest.slice(modifier.length);
      }
    }
    const arrows = { up: "↑", down: "↓", left: "←", right: "→" };
    const base = names[rest] ?? arrows[rest.toLowerCase()] ?? (rest.length === 1 ? rest.toUpperCase() : rest);
    return out + base;
  }).join("/");
}

function keycap(t, label) {
  return div().flex_none().px(5).h(18).flex().items_center().rounded(4).bg(t.raised).border_1().border_color(t.border)
    .text_size(10.5).text_color(t.muted).child(keyLabel(label));
}

// The picker's key hints. Leaving is always Esc in the header, so its hints are left out.
function hints(t, list) {
  const shown = list.filter((h) => h.label !== "quit" && h.label !== "clear/quit");
  return h_flex().flex_wrap().gap_x(14).gap(4).items_center().text_size(11.5).text_color(t.dim)
    .children(shown.map((h) => h_flex().gap(6).items_center().child(keycap(t, h.key)).child(h.label)));
}

// How each badge reads in a row. The main checkout goes without saying.
const BADGES = {
  main_checkout: null,
  folder: (t) => ["folder", t.muted],
  worktree: (t) => ["worktree", t.accent],
  current: (t) => ["current", t.done],
  default: (t) => ["default", t.muted],
  remote: (t, b) => [b.name, t.muted],
};

function shownBadges(badges) {
  return badges.filter((b) => BADGES[b.kind] !== null);
}

function badge(t, b) {
  const [label, color] = (BADGES[b.kind] ?? ((tt) => [b.kind, tt.muted]))(t, b);
  return div().flex_none().h(18).px(6).flex().items_center().rounded(5).border_1().border_color(t.border)
    .text_size(10.5).text_color(color).child(label);
}

// The search field: the picker owns the text, so the mod draws it and, while typing goes to
// it (`active`), its cursor.
function field(t, query, placeholder, active = true) {
  const empty = !query.before && !query.after;
  const cursor = active
    ? div().w(1.5).h(16).flex_none().bg(t.accent).loop("opacity", { from: 0.15, to: 1, period: 1100 })
    : div();
  return h_flex().h(38).flex_none().px(INSET).gap(ICON_GAP).items_center().rounded(9).border_1()
    .border_color(active ? t.accent : t.border).bg(t.background).text_size(13.5)
    .child(icon(t, "search", t.dim, ICON))
    .child(
      h_flex().flex_1().min_w(0).overflow_hidden().items_center().whitespace_nowrap()
        .child(div().flex_none().text_color(t.foreground).child(query.before))
        .child(cursor)
        .when(empty, (el) => el.child(div().pl(1).truncate().text_color(t.dim).child(placeholder)))
        .when(!empty, (el) => el.child(div().flex_none().text_color(t.foreground).child(query.after))),
    );
}

// A dialog's card. It keeps its clicks, so they don't reach the backdrop behind it.
function card(t, width) {
  return v_flex().id("kiosk-card").on_click((_e, cx) => cx.stop_propagation()).w(width).max_w("92%").p(CARD_EDGE).gap(12).rounded(12).bg(t.popover).border_1().border_color(t.border)
    .shadow_lg().text_size(13);
}

// What covers the picker behind a dialog: `dismiss` runs on a click beside the dialog.
function backdrop(t, child, dismiss) {
  return div().id("kiosk-backdrop").absolute().inset(0).bg(`${t.canvas}d9`).flex().items_center().justify_center()
    .on_click((_e, cx) => { cx.stop_propagation(); dismiss(); })
    .child(child);
}

function heading(t, text, size = 14) {
  return div().text_size(size).font_semibold().text_color(t.foreground).child(text);
}

// The help entries in view with each command's keys on one row, as a shortcuts list reads.
function helpRows(h) {
  const rows = [];
  h.entries.forEach((entry, i) => {
    const previous = rows[rows.length - 1];
    if (entry.kind !== "binding") {
      rows.push(entry);
    } else if (previous?.kind === "binding" && previous.command === entry.command) {
      previous.keys.push(entry.key);
      previous.positions.push(entry.position);
      previous.ids.push(entry.id);
      previous.selected ||= i === h.selected;
    } else {
      rows.push({ ...entry, keys: [entry.key], positions: [entry.position], ids: [entry.id], selected: i === h.selected });
    }
  });
  return rows;
}

// What a list is called, as the app would name it.
function listTitle(list) {
  switch (list.kind) {
    case "repos":
      return { title: "Repositories" };
    case "branches":
      return { parent: "Repositories", title: list.subject ?? "Branches" };
    default:
      return { title: list.title };
  }
}

export default class Kiosk extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.timer.after(600, () => this.handle.focus());
    this.view = null;
    this.chords = [];
    this.writes = Promise.resolve();
    this.wheel = 0;
    this.status = "Starting…";
    // `herdr-app check` loads the mod with no overlay; only a shown copy starts the picker.
    if (!props()?.overlay) return;
    cx.spawn(async (cx) => {
      try {
        const { socket, token } = await start();
        this.socket = socket;
        this.send({ t: "hello", token, protocol: PROTOCOL });
        await this.listen(cx);
      } catch (error) {
        if (this.closing) return;
        const known = error instanceof KioskError;
        notify({ title: known ? error.title : "Kiosk stopped", body: known ? error.body : String(error), level: "error" });
        this.finish();
      }
    });
  }

  async listen(cx) {
    const lines = new Lines();
    for (;;) {
      let chunk;
      try {
        chunk = await this.socket.read();
      } catch (error) {
        // A read gives up after 30 s; the picker pings every 10, so a timeout means it's gone.
        throw new KioskError("Kiosk stopped responding", String(error));
      }
      if (chunk === null) {
        if (!this.closing) throw new KioskError("Kiosk stopped", "The picker closed its connection.");
        return;
      }
      for (const line of lines.push(chunk)) {
        if (this.receive(JSON.parse(line), cx)) return;
      }
      cx.notify();
    }
  }

  // Applies one message from the picker. Returns whether the session is over.
  receive(message, cx) {
    switch (message.t) {
      case "hello":
        this.chords = message.chords;
        cx.bind_keys(this.chords.map((keystroke) => ({ keystroke, action: `kiosk.key:${keystroke}`, context: CONTEXT })));
        this.send({ t: "viewport", ...VIEWPORT });
        return false;
      case "view":
        this.view = message.view;
        return false;
      case "notify":
        notify({ title: message.title, body: message.body, level: message.level });
        return false;
      case "error":
        notify({ title: message.title, body: message.body, level: "error" });
        this.finish();
        return true;
      case "exit":
        this.finish();
        return true;
      case "ping":
        return false;
      default:
        throw new KioskError("Kiosk stopped", `The picker sent a message this mod doesn't know: ${JSON.stringify(message)}`);
    }
  }

  send(message) {
    const socket = this.socket;
    if (!socket || this.closing) return;
    const line = `${JSON.stringify(message)}\n`;
    // One write at a time, so the picker sees the person's input in order. A write that
    // fails loses input, so the picker can no longer be trusted to show what was done.
    this.writes = this.writes.then(() => socket.write(line)).catch((error) => {
      if (this.closing) return;
      notify({ title: "Kiosk stopped responding", body: `Sending to the picker failed: ${error}`, level: "error" });
      this.finish();
    });
  }

  finish() {
    if (this.closing) return;
    this.closing = true;
    this.socket?.close();
    closeOverlay(props().overlay);
  }

  async close() {
    if (this.closing) return;
    this.closing = true;
    const socket = this.socket;
    if (!socket) return;
    try {
      await this.writes;
      await socket.write(`${JSON.stringify({ t: "close" })}\n`);
    } catch {}
    socket.close();
  }

  key(e, cx) {
    const mac = window().platform === "macos";
    const pasting = e.key === "v" && (mac ? e.modifiers.platform && !e.modifiers.control : e.modifiers.control && !e.modifiers.platform);
    if (pasting || (e.key === "insert" && e.modifiers.shift)) {
      const text = cx.read_from_clipboard();
      if (text && Buffer.from(text).length > MAX_PASTE) {
        notify({ title: "Kiosk didn't paste", body: "That is too much text to search for.", level: "warning" });
      } else if (text) {
        this.send({ t: "paste", text });
      }
      cx.stop_propagation();
      return;
    }
    // The platform key is the app's; the picker binds none.
    if (e.modifiers.platform) return;
    this.send({ t: "key", keystroke: e.keystroke, key_char: e.key_char ?? null });
    cx.stop_propagation();
  }

  scroll(e, cx) {
    this.wheel -= (e.delta_lines?.y ?? e.delta.y / ROW);
    const rows = Math.trunc(this.wheel);
    if (rows !== 0) {
      this.wheel -= rows;
      this.send({ t: "scroll", delta: rows });
    }
    cx.stop_propagation();
  }

  render() {
    const t = theme();
    let root = v_flex().id("kiosk").key_context(CONTEXT).track_focus(this.handle).size_full().min_h(0).relative()
      .bg(t.popover).text_size(13).text_color(t.foreground)
      .on_key_down((e, cx) => this.key(e, cx));
    for (const keystroke of this.chords) {
      root = root.on_action(`kiosk.key:${keystroke}`, () => this.send({ t: "key", keystroke, key_char: null }));
    }
    const v = this.view;
    if (!v) return root.child(this.starting(t));
    if (v.screen === "setup") return root.child(this.setup(t, v));
    if (v.screen === "no_search_dirs") return root.child(this.noSearchDirs(t, v));
    return root.child(this.picker(t, v));
  }

  starting(t) {
    return v_flex().size_full().p(EDGE).gap(10)
      .child(this.header(t, { title: TITLE }))
      .child(h_flex().flex_1().items_center().justify_center().gap(8).py(40)
        .child(spinner(t)).child(div().text_color(t.dim).child(this.status)));
  }

  // A button for one of the picker's commands; `primary` for the one Enter does.
  button(t, id, label, command, primary = false) {
    return h_flex().id(`kiosk-button-${id}`).flex_none().h(28).px(12).rounded(7).items_center().text_size(12.5).cursor_pointer()
      .when(primary, (el) => el.bg(t.solid).text_color(t.solid_foreground).font_semibold().hover((s) => s.opacity(0.88)))
      .when(!primary, (el) => el.border_1().border_color(t.border).text_color(t.foreground).hover((s) => s.bg(t.hover)))
      .when(command !== null, (el) => el.on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "command", command }); }))
      .child(label);
  }

  dismissButton(t) {
    return this.button(t, "cancel", "Cancel", null)
      .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "dismiss" }); });
  }

  // A setup wizard button: the wizard is driven by keys, so it presses `keystroke`.
  keyButton(t, id, label, keystroke, primary = false) {
    return this.button(t, id, label, null, primary)
      .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "key", keystroke, key_char: null }); });
  }

  buttons(t, ...children) {
    return h_flex().flex_none().pt(4).gap(8).justify_end().children(children);
  }

  // A footer of hints with buttons at its end.
  actions(t, list, ...buttons) {
    return h_flex().flex_none().pt(4).items_center().gap(12)
      .child(div().flex_1().min_w(0).child(hints(t, list)))
      .child(this.buttons(t, ...buttons));
  }

  // The title row: `name.parent` before the title goes back to it; `right` sits before Esc.
  header(t, name, right) {
    const crumb = name.parent
      ? h_flex().id("kiosk-back").flex_none().gap(4).items_center().rounded(6).px(4).ml(-7).mr(-4).h(24).cursor_pointer()
        .text_color(t.dim).hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "command", command: "repositories" }); })
        .child(icon(t, "chevron-left", t.dim, 13))
        .child(div().text_size(14).child(name.parent))
      : null;
    return h_flex().flex_none().h(24).items_center().gap(6)
      .when(!!crumb, (el) => el.child(crumb).child(div().text_size(14).text_color(t.dim).child("/")))
      .child(div().flex_1().min_w(0).truncate().text_size(14).font_semibold().text_color(t.foreground).child(name.title))
      .when(!!right, (el) => el.child(right))
      .child(
        div().id("kiosk-close").flex_none().ml(4).px(6).h(20).flex().items_center().rounded(5).text_size(11).text_color(t.dim).cursor_pointer()
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click((_e, cx) => { cx.stop_propagation(); this.finish(); })
          .child("Esc"),
      );
  }

  picker(t, v) {
    const main = v.main;
    const covered = !!v.dialog || !!v.overlay;
    let content;
    if (main.state === "loading") {
      content = v_flex().size_full().p(EDGE).gap(10)
        .child(this.header(t, { title: TITLE }))
        .child(
          v_flex().flex_1().items_center().justify_center().gap(8).py(40)
            .child(h_flex().gap(8).items_center().child(spinner(t, t.accent)).child(div().text_color(t.foreground).child(main.message)))
            .when(!!main.hint, (el) => el.child(div().text_size(12).text_color(t.dim).child(main.hint))),
        );
    } else {
      content = v_flex().size_full().gap(10).pt(EDGE)
        .child(div().px(EDGE).child(this.header(t, listTitle(main), this.summary(t, main))))
        .child(div().px(EDGE).child(field(t, main.query, main.placeholder, !covered)))
        .child(div().px(EDGE).child(this.list(t, main, "list", VIEWPORT.list, (row, selected) => this.row(t, row, selected, !covered))))
        .when(!!v.toast, (el) => el.child(div().px(EDGE).child(this.toast(t, v.toast))))
        .child(h_flex().flex_none().px(EDGE).h(40).items_center().border_t(1).border_color(t.border).child(hints(t, v.hints)));
    }
    let screen = div().size_full().relative().child(content);
    const dismiss = () => this.send({ t: "dismiss" });
    if (v.dialog) screen = screen.child(backdrop(t, this.dialog(t, v.dialog, v.hints), dismiss));
    if (v.overlay?.kind === "help") screen = screen.child(backdrop(t, this.help(t, v.overlay), dismiss));
    if (v.overlay?.kind === "scan_warnings") screen = screen.child(backdrop(t, this.scanWarnings(t, v.overlay), dismiss));
    return screen;
  }

  summary(t, list) {
    return h_flex().flex_none().gap(6).items_center().text_size(11.5).text_color(t.dim)
      .when(!!list.activity, (el) => el.child(spinner(t)).child(`${list.activity}…`).child(div().text_color(t.border).child("·")))
      .child(list.summary);
  }

  // A list of the picker's rows: `rows` of them, each one row tall.
  list(t, list, name, rows, row) {
    const area = v_flex().id(`kiosk-${name}`).h(rows * ROW).flex_none().overflow_hidden()
      .on_scroll_wheel((e, cx) => this.scroll(e, cx));
    if (!list.rows.length) {
      return area.child(
        v_flex().h(rows * ROW).items_center().justify_center().gap(8).text_color(t.dim)
          .child(icon(t, list.kind === "repos" ? "folder" : "git-branch", t.dim, 18))
          .child(div().text_size(12.5).child(list.empty ?? "")),
      );
    }
    return area.children(list.rows.map((r, i) => row(r, i === list.selected)));
  }

  // `interactive`: whether the row takes the pointer, which it doesn't under a dialog.
  row(t, r, selected, interactive) {
    const id = r.id;
    const hovered = interactive && this.hovered === id;
    const folder = r.badges.some((b) => b.kind === "folder");
    const glyph = r.actions.branches || folder ? "folder" : "git-branch";
    const action = (name, iconName, label, command) =>
      h_flex().id(`kiosk-${name}-${id}`).flex_none().gap(5).px(7).h(22).rounded(6).items_center().text_size(11.5).text_color(t.muted).cursor_pointer()
        .hover((s) => s.bg(t.raised).text_color(t.foreground))
        .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "activate", position: r.position, id: r.id, command }); })
        .child(icon(t, iconName, null, 12))
        .child(label);
    return h_flex().id(`kiosk-row-${id}`).h(ROW).flex_none().px(ROW_INSET).rounded(7).gap(ICON_GAP).items_center()
      .when(selected, (el) => el.bg(t.selected))
      .when(interactive, (el) => el.cursor_pointer()
        .when(!selected, (row) => row.hover((s) => s.bg(t.hover)))
        .on_hover((on, cx) => {
          if (on) this.hovered = id;
          else if (this.hovered === id) this.hovered = null;
          cx.notify();
        })
        .on_click(() => this.send({ t: "activate", position: r.position, id: r.id })))
      .child(icon(t, glyph, selected ? t.muted : t.dim, ICON))
      .child(div().flex_none().max_w("60%").truncate().text_color(toneColor(t, r.name.tone)).when(selected, (el) => el.font_medium()).child(r.name.text))
      .children(shownBadges(r.badges).map((b) => badge(t, b)))
      .child(div().flex_1())
      .children(r.labels.map((l) => div().flex_none().text_size(11.5).text_color(toneColor(t, l.tone)).child(l.text)))
      .when(r.open && !hovered, (el) => el.child(h_flex().flex_none().gap(5).items_center().text_size(11.5).text_color(t.muted)
        .child(div().size(6).rounded(3).bg(t.done)).child("open")))
      .when(hovered && r.actions.branches, (el) => el.child(action("branches", "git-branch", "Branches", "branches")))
      .when(hovered && r.actions.delete, (el) => el.child(action("delete", "x", "Remove", "delete")));
  }

  toast(t, toast) {
    const color = toneColor(t, toast.level);
    return h_flex().flex_none().gap(ICON_GAP).px(INSET).py(8).rounded(9).items_center().bg(t.raised).border_1().border_color(t.border)
      .child(div().flex_none().size(7).rounded(4).bg(color))
      .child(div().flex_1().min_w(0).text_size(12.5).text_color(t.foreground).child(toast.message))
      .when(!!toast.counter, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(toast.counter)))
      .child(hints(t, toast.hints));
  }

  // `footer` is the picker's hints, which the dialog covers.
  dialog(t, d, footer) {
    switch (d.kind) {
      case "validating_branch":
        return card(t, 400)
          .child(heading(t, d.title))
          .child(h_flex().gap(8).items_center().child(spinner(t, t.accent)).child(div().text_color(t.muted).child(d.message)));
      case "base_branch":
        return card(t, 520)
          .child(v_flex().gap(3)
            .child(h_flex().gap(6).items_center().child(heading(t, "New branch"))
              .when(!!d.list.subject, (el) => el.child(div().text_size(14).font_semibold().text_color(t.accent).child(d.list.subject))))
            .child(div().text_size(12).text_color(t.dim).child("Pick the branch to start it from")))
          .child(field(t, d.list.query, d.list.placeholder))
          .child(this.list(t, d.list, "bases", VIEWPORT.bases, (row, selected) => this.row(t, row, selected, true)))
          .child(this.buttons(t, this.dismissButton(t), this.button(t, "create", "Create branch", "open", true)));
      case "remote_branch":
        return card(t, 500)
          .child(heading(t, d.title))
          .children(d.lines.map((line) => div().text_color(t.muted).child(line)))
          .child(
            v_flex().gap(2).children(d.choices.map((choice, i) =>
              h_flex().id(`kiosk-choice-${i}`).px(ROW_INSET).h(ROW).gap(ICON_GAP).rounded(7).items_center().cursor_pointer()
                .when(i === d.selected, (el) => el.bg(t.selected))
                .when(i !== d.selected, (el) => el.hover((s) => s.bg(t.hover)))
                .on_click(() => this.send({ t: "activate", position: i, id: choice.id }))
                .child(icon(t, "git-branch", i === d.selected ? t.muted : t.dim, ICON))
                .child(choice.label))),
          )
          .child(this.buttons(t, this.dismissButton(t), this.button(t, "continue", "Continue", "open", true)));
      case "confirm_delete":
        return card(t, 500)
          .child(heading(t, d.force ? "Force-remove this checkout?" : "Remove this checkout?"))
          .when(!!d.warning, (el) => el.child(h_flex().gap(8).items_center().text_size(12.5).text_color(t.blocked)
            .child(div().size(7).rounded(4).bg(t.blocked)).child(d.warning)))
          .child(h_flex().px(INSET).h(ROW).gap(ICON_GAP).items_center().rounded(9).bg(t.background).border_1().border_color(t.border)
            .child(icon(t, "folder", t.dim, ICON))
            .child(div().flex_1().min_w(0).truncate().text_size(12.5).text_color(t.foreground).child(d.path)))
          .child(div().text_size(12.5).text_color(t.muted).child(d.closes_in
            ? `Its space in ${d.closes_in} closes too. The branch stays.`
            : "The branch stays."))
          .child(d.busy
            ? h_flex().pt(4).gap(8).items_center().child(spinner(t, t.accent)).child(div().text_color(t.muted).child(d.busy))
            : this.buttons(t, this.dismissButton(t), this.button(t, "remove", d.force ? "Force-remove" : "Remove", "open", true)));
      default:
        return card(t, 400).child(div().text_color(t.ansi1).child(`Unknown dialog ${d.kind}`));
    }
  }

  help(t, h) {
    return card(t, 600)
      .child(h_flex().items_center().child(div().flex_1().child(heading(t, "Keyboard shortcuts"))).child(div().text_size(11.5).text_color(t.dim).child(h.summary.replace(" (esc: close)", ""))))
      .child(field(t, h.query, h.placeholder))
      .when(!!h.notice, (el) => el.child(div().text_size(11.5).text_color(t.muted).child(h.notice)))
      .child(
        v_flex().id("kiosk-help").h(VIEWPORT.help * HELP_ROW).flex_none().overflow_hidden()
          .on_scroll_wheel((e, cx) => this.scroll(e, cx))
          .when(!!h.empty, (el) => el.child(div().p(12).text_color(t.dim).child(h.empty)))
          .children(helpRows(h).map((entry) => {
            const line = h_flex().h(HELP_ROW).flex_none().items_center().px(ROW_INSET);
            if (entry.kind === "section") return line.text_size(10.5).font_semibold().text_color(t.dim).child(entry.name.toUpperCase());
            if (entry.kind === "blank") return line;
            return line.id(`kiosk-help-${entry.positions[0]}`).gap(12).rounded(6).cursor_pointer()
              .when(entry.selected, (el) => el.bg(t.selected))
              .when(!entry.selected, (el) => el.hover((s) => s.bg(t.hover)))
              .on_click(() => this.send({ t: "activate", position: entry.positions[0], id: entry.ids[0] }))
              .child(div().flex_1().min_w(0).truncate().text_size(12.5).child(entry.description))
              .child(h_flex().flex_none().gap(4).justify_end().children(entry.keys.map((k) => keycap(t, k))));
          })),
      );
  }

  scanWarnings(t, s) {
    return card(t, 600)
      .child(h_flex().gap(8).items_center().child(div().size(7).rounded(4).bg(t.blocked)).child(heading(t, s.title)))
      .child(
        v_flex().id("kiosk-warnings").h(VIEWPORT.scan_warnings * WARNING_ROW).flex_none().overflow_hidden()
          .on_scroll_wheel((e, cx) => this.scroll(e, cx))
          .children(s.warnings.map((w) =>
            v_flex().h(WARNING_ROW).flex_none().justify_center().gap(2)
              .child(div().truncate().text_size(12.5).child(w.path))
              .child(div().truncate().text_size(11.5).text_color(t.muted).child(w.message)))),
      )
      .child(hints(t, s.hints));
  }

  setup(t, v) {
    const page = (...children) =>
      v_flex().size_full().p(EDGE).gap(14)
        .child(this.header(t, { title: `Set up ${TITLE}` }))
        .children(children);
    const label = (text) => div().text_size(11).font_semibold().text_color(t.dim).child(text.toUpperCase());
    const added = (dirs) =>
      v_flex().gap(2).children(dirs.map((dir) =>
        h_flex().h(28).px(ROW_INSET).gap(ICON_GAP).items_center()
          .child(icon(t, "folder", t.dim, ICON))
          .child(div().flex_1().min_w(0).truncate().child(dir.path))
          .child(div().text_size(11.5).text_color(t.dim).child(dir.depth))
          .when(dir.all_folders, (el) => el.child(badge(t, { kind: "folder" })))));
    switch (v.step) {
      case "welcome":
        return page(
          v_flex().flex_1().justify_center().gap(8).py(16)
            .child(div().text_size(18).font_semibold().child(v.heading))
            .child(div().text_color(t.muted).child(v.intro))
            .child(div().text_color(t.muted).child(v.explanation))
            .child(h_flex().pt(6).gap(4).text_size(12).child(div().text_color(t.dim).child(v.config_label)).child(div().truncate().text_color(t.muted).child(v.config_path))),
          this.actions(t, v.hints, this.keyButton(t, "quit", "Quit", "escape"), this.keyButton(t, "continue", "Get started", "enter", true)));
      case "directories":
        return page(
          div().text_color(t.muted).child(v.instructions),
          v_flex().gap(6)
            .child(label(v.path.label))
            .child(v.depth
              ? h_flex().h(38).px(INSET).gap(ICON_GAP).items_center().rounded(9).border_1().border_color(t.border).bg(t.background).text_color(t.muted)
                .child(icon(t, "folder", t.dim, ICON)).child(v.path.query.before + v.path.query.after)
              : field(t, v.path.query, v.path.placeholder)),
          v.depth
            ? v_flex().gap(6).child(label(v.depth.label)).child(field(t, v.depth.query, v.depth.placeholder))
            : v_flex().gap(1).children(v.completions.map((completion, i) =>
              h_flex().id(`kiosk-completion-${i}`).px(ROW_INSET).h(28).gap(ICON_GAP).rounded(7).items_center().cursor_pointer().text_size(12.5).text_color(t.muted)
                .when(i === v.selected_completion, (el) => el.bg(t.selected).text_color(t.foreground))
                .when(i !== v.selected_completion, (el) => el.hover((s) => s.bg(t.hover)))
                .on_click(() => this.send({ t: "completion", index: i }))
                .child(icon(t, "folder", t.dim, ICON))
                .child(completion))),
          v_flex().flex_1().min_h(0).gap(6)
            .child(label(v.added_title))
            .child(v.added.length ? added(v.added) : div().text_size(12.5).text_color(t.dim).child(v.nothing_added)),
          v.message
            ? h_flex().gap(8).items_center().text_color(t.blocked).child(div().size(7).rounded(4).bg(t.blocked)).child(v.message)
            : this.actions(t, v.hints, v.depth
              ? this.keyButton(t, "add", "Add directory", "enter", true)
              : this.keyButton(t, "done", v.added.length && !v.path.query.before ? "Continue" : "Add", "enter", true)));
      case "folder_mode":
        return page(
          v_flex().gap(4)
            .child(heading(t, v.question))
            .child(h_flex().gap(6).items_center().text_size(12).text_color(t.dim).child(icon(t, "folder", t.dim, 12)).child(v.directory).child("·").child(v.depth)),
          v_flex().gap(6).children(v.options.map((option) =>
            h_flex().id(`kiosk-mode-${option.mode}`).px(INSET).h(40).gap(ICON_GAP).rounded(9).items_center().cursor_pointer().border_1()
              .border_color(option.selected ? t.accent : t.border)
              .when(option.selected, (el) => el.bg(t.selected))
              .when(!option.selected, (el) => el.hover((s) => s.bg(t.hover)))
              .on_click(() => this.send({ t: "folder_mode", mode: option.mode }))
              .child(div().size(ICON).flex_none().rounded(8).border_1().border_color(option.selected ? t.accent : t.border).flex().items_center().justify_center()
                .when(option.selected, (el) => el.child(div().size(6).rounded(3).bg(t.accent))))
              .child(option.label))),
          div().flex_1(),
          this.actions(t, v.hints, this.keyButton(t, "back", "Back", "escape"), this.keyButton(t, "confirm", "Confirm", "enter", true)));
      case "confirm":
        return page(
          heading(t, v.heading, 16),
          added(v.dirs),
          h_flex().gap(4).text_size(12).child(div().text_color(t.dim).child(v.write_label)).child(div().truncate().text_color(t.muted).child(v.config_path)),
          div().flex_1(),
          this.actions(t, v.hints, this.keyButton(t, "back", "Back", "escape"), this.keyButton(t, "write", "Save and open", "enter", true)));
      default:
        return page(div().text_color(t.ansi1).child(`Unknown setup step ${v.step}`));
    }
  }

  noSearchDirs(t, v) {
    return v_flex().size_full().p(EDGE).gap(10)
      .child(this.header(t, { title: TITLE }))
      .child(v_flex().flex_1().items_center().justify_center().gap(8).py(32)
        .child(icon(t, "folder", t.dim, 20))
        .child(heading(t, v.title))
        .child(div().text_size(12.5).text_color(t.muted).child(v.location))
        .child(div().text_size(12).text_color(t.dim).child(v.hint)));
  }
}
