// A tiny fake shell per pane, kept as logical lines of styled cells and sent as
// screens of rows (`Screen` events), the way the link sends a real emulator's.

import type { InputItem, Mouse, Screen, ScreenLine, ScreenRow, Span } from '../src/api/gen/api.ts';

type Style = Omit<Span, 'text'>;

interface Cell {
  c: string;
  /** The style, as JSON (cells with the same key merge into one span). */
  s: string;
}

type Line = Cell[];

const PLAIN = '{}';
const MAX_LINES = 2000;

function styleKey(s: Style): string {
  return JSON.stringify(s);
}

function cells(text: string, style: Style = {}): Line {
  const s = styleKey(style);
  return Array.from(text, (c) => ({ c, s }));
}

/** A line from parts: plain strings or `[text, style]`. */
function line(...parts: (string | [string, Style])[]): Line {
  return parts.flatMap((p) => (typeof p === 'string' ? cells(p) : cells(p[0], p[1])));
}

function toSpans(row: Cell[]): Span[] {
  let end = row.length;
  while (end > 0 && row[end - 1]!.c === ' ' && row[end - 1]!.s === PLAIN) end--;
  const spans: Span[] = [];
  for (let i = 0; i < end; i++) {
    const cell = row[i]!;
    const last = spans[spans.length - 1] as (Span & { key?: string }) | undefined;
    if (last && last.key === cell.s) last.text += cell.c;
    else spans.push({ text: cell.c, ...(JSON.parse(cell.s) as Style), key: cell.s } as Span);
  }
  for (const s of spans) delete (s as { key?: string }).key;
  return spans;
}

export interface ShellOptions {
  user: string;
  host: string;
  cwd: string;
  home: string;
  /** Lines already on the screen, as if typed earlier. */
  seed?: { command: string; output: string[] }[];
}

type Emit = (screen: Screen) => void;

export class Shell {
  cols = 80;
  rows = 24;
  private readonly opts: ShellOptions;
  private history: Line[] = [];
  private input = '';
  private cursor = 0;
  private past: string[] = [];
  private pastIndex = -1;
  /** The index in `history` of the line a `clear` put at the top. */
  private clearAt = 0;
  /** `top`: it asks for the mouse; a click or the wheel picks a process. */
  private alt: { timer: ReturnType<typeof setInterval>; tick: number; picked: number; mouse: string } | null = null;
  private readonly subs = new Set<Emit>();
  private prevRows: string[] = [];
  private prevTop = 0;
  private needFull = true;
  private timer: ReturnType<typeof setTimeout> | null = null;
  title: string;

  constructor(opts: ShellOptions) {
    this.opts = opts;
    this.title = `${opts.user}@${opts.host}: ${this.shortCwd()}`;
    for (const s of opts.seed ?? []) {
      this.history.push([...this.prompt(), ...cells(s.command)]);
      for (const o of s.output) this.history.push(cells(o));
    }
  }

  // ---------------------------------------------------------------- watching

  subscribe(emit: Emit): () => void {
    if (!this.subs.size && !this.timer) {
      this.remember();
      this.needFull = false;
    }
    this.subs.add(emit);
    emit(this.snapshot(true));
    return () => this.subs.delete(emit);
  }

  /** The last lines of the screen, as text, for a card's preview. */
  lastLines(n: number): string[] {
    const { rows, top } = this.layout();
    const screen = rows.slice(top, top + this.rows).map((r) => r.cells.map((c) => c.c).join('').trimEnd());
    while (screen.length && screen[screen.length - 1] === '') screen.pop();
    return screen.slice(-n);
  }

  resize(cols: number, rows: number): void {
    this.cols = Math.max(10, Math.min(500, cols));
    this.rows = Math.max(3, Math.min(200, rows));
    this.needFull = true;
    this.dirty();
  }

  /** Lines of scrollback, oldest first; `from` counts back from the screen's top. */
  scrollback(from: number, count: number): ScreenLine[] {
    const { rows, top } = this.layout();
    const end = Math.max(0, top - from);
    const start = Math.max(0, end - count);
    return rows.slice(start, end).map((r) => ({ spans: toSpans(r.cells), wraps: r.wraps }));
  }

  /** Prints a line above the prompt (a watcher's output). */
  print(text: string, style: Style = {}): void {
    this.history.push(cells(text, style));
    this.trim();
    this.dirty();
  }

  // ---------------------------------------------------------------- input

  feed(items: InputItem[]): void {
    for (const item of items) {
      switch (item.type) {
        case 'Text':
          for (const ch of item.content) this.typed(ch);
          break;
        case 'Paste': {
          const parts = item.content.replace(/\r\n?/g, '\n').split('\n');
          parts.forEach((p, i) => {
            for (const ch of p) this.typed(ch);
            if (i < parts.length - 1) this.enter();
          });
          break;
        }
        case 'Key':
          this.key(item.content.key, item.content.ctrl ?? false, item.content.alt ?? false);
          break;
        case 'Mouse':
          // Only a program that asked for the mouse gets it, as with the link.
          if (this.alt) this.mouse(item.content);
          break;
      }
    }
    this.dirty();
  }

  /** Runs a command as if typed (a command tab). */
  run(command: string): void {
    this.input = command;
    this.cursor = command.length;
    this.enter();
    this.dirty();
  }

  private typed(ch: string): void {
    if (ch === '\r' || ch === '\n') return this.enter();
    if (ch === '\x7f' || ch === '\b') return this.key('Backspace', false, false);
    if (ch === '\x03') return this.key('c', true, false);
    if (this.alt) {
      if (ch === 'q') this.leaveAlt();
      return;
    }
    if (ch < ' ') return;
    this.input = this.input.slice(0, this.cursor) + ch + this.input.slice(this.cursor);
    this.cursor += ch.length;
  }

  private key(key: string, ctrl: boolean, alt: boolean): void {
    if (this.alt) {
      if (key === 'q' || (ctrl && key === 'c') || key === 'Escape') this.leaveAlt();
      return;
    }
    if (ctrl) {
      switch (key.toLowerCase()) {
        case 'c':
          this.history.push([...this.prompt(), ...cells(this.input), ...cells('^C')]);
          this.input = '';
          this.cursor = 0;
          return;
        case 'l':
          this.clearAt = this.history.length;
          return;
        case 'a':
          this.cursor = 0;
          return;
        case 'e':
          this.cursor = this.input.length;
          return;
        case 'u':
          this.input = this.input.slice(this.cursor);
          this.cursor = 0;
          return;
        case 'd':
          return;
        default:
          return;
      }
    }
    if (alt) return;
    switch (key) {
      case 'Enter':
        return this.enter();
      case 'Backspace':
        if (this.cursor > 0) {
          this.input = this.input.slice(0, this.cursor - 1) + this.input.slice(this.cursor);
          this.cursor--;
        }
        return;
      case 'Delete':
        this.input = this.input.slice(0, this.cursor) + this.input.slice(this.cursor + 1);
        return;
      case 'Left':
        this.cursor = Math.max(0, this.cursor - 1);
        return;
      case 'Right':
        this.cursor = Math.min(this.input.length, this.cursor + 1);
        return;
      case 'Home':
        this.cursor = 0;
        return;
      case 'End':
        this.cursor = this.input.length;
        return;
      case 'Up':
        if (!this.past.length) return;
        this.pastIndex = this.pastIndex < 0 ? this.past.length - 1 : Math.max(0, this.pastIndex - 1);
        this.input = this.past[this.pastIndex]!;
        this.cursor = this.input.length;
        return;
      case 'Down':
        if (this.pastIndex < 0) return;
        this.pastIndex++;
        if (this.pastIndex >= this.past.length) {
          this.pastIndex = -1;
          this.input = '';
        } else this.input = this.past[this.pastIndex]!;
        this.cursor = this.input.length;
        return;
      case 'Tab':
      case 'Escape':
        return;
      default:
        if (key.length === 1) this.typed(key);
    }
  }

  private enter(): void {
    const command = this.input;
    this.history.push([...this.prompt(), ...cells(command)]);
    this.input = '';
    this.cursor = 0;
    this.pastIndex = -1;
    if (command.trim()) {
      this.past.push(command);
      this.exec(command.trim());
    }
    this.trim();
  }

  // ---------------------------------------------------------------- commands

  private out(...lines: Line[]): void {
    this.history.push(...lines);
  }

  private exec(command: string): void {
    const [name, ...args] = command.split(/\s+/);
    switch (name) {
      case 'ls':
        this.out(
          line(
            ['crates', { fg: '4', bold: true }],
            '  ',
            ['docs', { fg: '4', bold: true }],
            '  ',
            ['web', { fg: '4', bold: true }],
            '  Cargo.toml  Cargo.lock  README.md  ',
            ['build.sh', { fg: '2', bold: true }],
          ),
        );
        break;
      case 'echo':
        this.out(cells(args.join(' ')));
        break;
      case 'date':
        this.out(cells(new Date().toString()));
        break;
      case 'pwd':
        this.out(cells(this.opts.cwd));
        break;
      case 'clear':
        this.clearAt = this.history.length;
        break;
      case 'help':
        for (const l of [
          'A pretend shell, for trying the web app:',
          '  ls, echo, date, pwd, clear, help',
          '  seq N     N lines (scrollback)',
          '  long      a very long line (wrapping)',
          '  colors    the 16 colours and styles',
          '  top       a full-screen program, with the mouse (q quits)',
        ])
          this.out(cells(l));
        break;
      case 'seq': {
        const n = Math.min(5000, Math.max(1, Number(args[0] ?? 10) || 10));
        for (let i = 1; i <= n; i++) this.out(cells(String(i)));
        break;
      }
      case 'long': {
        let s = '';
        for (let i = 0; s.length < 300; i++) s += `word${i} `;
        this.out(cells(s.slice(0, 300)));
        break;
      }
      case 'colors': {
        const row = (from: number): Line =>
          line(...Array.from({ length: 8 }, (_, i): [string, Style] => [` ${String(from + i).padStart(2)} `, { bg: String(from + i), fg: from + i === 0 ? '7' : '0' }]));
        this.out(row(0), row(8));
        this.out(
          line(['bold', { bold: true }], ' ', ['dim', { dim: true }], ' ', ['italic', { italic: true }], ' ', ['underline', { underline: true }], ' ', ['strike', { strike: true }], ' ', ['inverse', { inverse: true }], ' ', ['#FF8A57', { fg: '#ff8a57' }]),
        );
        break;
      }
      case 'top':
        this.enterAlt();
        break;
      case 'cd':
        break;
      default:
        this.out(cells(`${name}: command not found`));
    }
  }

  private mouse(m: Mouse): void {
    const alt = this.alt!;
    const first = 4;
    const procs = this.rows - 1 - first;
    alt.mouse = `${m.kind}${m.button ? ' ' + m.button : ''} at ${m.col},${m.row}`;
    if (m.kind === 'press' && m.row >= first && m.row < first + procs) alt.picked = m.row - first;
    if (m.kind === 'wheelUp') alt.picked = Math.max(0, alt.picked - 1);
    if (m.kind === 'wheelDown') alt.picked = Math.min(procs - 1, alt.picked + 1);
  }

  private enterAlt(): void {
    this.alt = {
      picked: 0,
      mouse: '',
      tick: 0,
      timer: setInterval(() => {
        if (this.alt) this.alt.tick++;
        this.dirty();
      }, 1000),
    };
    this.title = 'top';
    this.needFull = true;
  }

  private leaveAlt(): void {
    if (!this.alt) return;
    clearInterval(this.alt.timer);
    this.alt = null;
    this.title = `${this.opts.user}@${this.opts.host}: ${this.shortCwd()}`;
    this.needFull = true;
  }

  private altRows(): Line[] {
    const tick = this.alt?.tick ?? 0;
    const head = line(
      [`top - ${new Date().toLocaleTimeString()} up 12 days,  load average: 0.${(tick * 7) % 100}, 0.42, 0.38`, {}],
    );
    const tasks = line('Tasks: 214 total,   1 running, 213 sleeping');
    const hdr = cells('    PID USER      %CPU  %MEM COMMAND'.padEnd(this.cols), { inverse: true });
    const procs = ['shedd', 'claude', 'cargo', 'rust-analyzer', 'node', 'bash', 'sshd', 'systemd'];
    const rows: Line[] = [head, tasks, cells(''), hdr];
    for (let i = 0; rows.length < this.rows - 1; i++) {
      const p = procs[i % procs.length]!;
      const cpu = ((Math.sin(tick + i) + 1) * 12).toFixed(1).padStart(5);
      const text = `${String(1000 + i * 37).padStart(7)} you      ${cpu}  ${(i * 0.7).toFixed(1).padStart(4)} ${p}`;
      rows.push(cells(i === this.alt?.picked ? text.padEnd(this.cols) : text, i === this.alt?.picked ? { bg: '4', fg: '0' } : {}));
    }
    const mouse = this.alt?.mouse ? `  ·  mouse: ${this.alt.mouse}` : '';
    rows.push(cells(`q: quit  ·  click or wheel: pick${mouse}`, { fg: '8' }));
    return rows.map((r) => r.slice(0, this.cols));
  }

  // ---------------------------------------------------------------- screen

  private prompt(): Line {
    return line([`${this.opts.user}@${this.opts.host}`, { fg: '2', bold: true }], ':', [this.shortCwd(), { fg: '4', bold: true }], '$ ');
  }

  private shortCwd(): string {
    const { cwd, home } = this.opts;
    return cwd === home ? '~' : cwd.startsWith(home + '/') ? '~' + cwd.slice(home.length) : cwd;
  }

  private trim(): void {
    const over = this.history.length - MAX_LINES;
    if (over > 0) {
      this.history.splice(0, over);
      this.clearAt = Math.max(0, this.clearAt - over);
    }
  }

  /** Every physical row (logical lines wrapped at the width), and where the screen starts. */
  private layout(): { rows: { cells: Cell[]; wraps: boolean }[]; top: number; cursorRow: number; cursorCol: number } {
    const all = [...this.history, [...this.prompt(), ...cells(this.input)]];
    const rows: { cells: Cell[]; wraps: boolean }[] = [];
    let clearRow = 0;
    let cursorRow = 0;
    let cursorCol = 0;
    all.forEach((l, i) => {
      if (i === this.clearAt) clearRow = rows.length;
      const start = rows.length;
      if (l.length === 0) rows.push({ cells: [], wraps: false });
      for (let o = 0; o < l.length; o += this.cols) {
        rows.push({ cells: l.slice(o, o + this.cols), wraps: o + this.cols < l.length });
      }
      if (i === all.length - 1) {
        const pos = this.prompt().length + this.cursor;
        cursorRow = start + Math.floor(pos / this.cols);
        cursorCol = pos % this.cols;
        while (rows.length <= cursorRow) rows.push({ cells: [], wraps: false });
        if (cursorRow > start) rows[cursorRow - 1]!.wraps = true;
      }
    });
    const top = Math.max(clearRow, rows.length - this.rows);
    return { rows, top, cursorRow: cursorRow - top, cursorCol };
  }

  private snapshot(full: boolean): Screen {
    if (this.alt) {
      const rows = this.altRows();
      const lines: ScreenRow[] = rows.map((r, i) => ({ row: i, line: { spans: toSpans(r), wraps: false } }));
      return {
        full: true,
        cols: this.cols,
        rows: this.rows,
        alt: true,
        lines,
        cursor: { row: 0, col: 0, visible: false },
        title: this.title,
        scrollback: 0,
        mouse: true,
        scrolled: 0,
      };
    }
    const { rows, top, cursorRow, cursorCol } = this.layout();
    const screen = rows.slice(top, top + this.rows);
    const encoded = screen.map((r) => JSON.stringify({ spans: toSpans(r.cells), wraps: r.wraps }));
    const lines: ScreenRow[] = [];
    for (let i = 0; i < this.rows; i++) {
      const enc = encoded[i] ?? JSON.stringify({ spans: [], wraps: false });
      const blank = enc === JSON.stringify({ spans: [], wraps: false });
      if (full ? !blank : this.prevRows[i] !== enc) lines.push({ row: i, line: JSON.parse(enc) as ScreenLine });
    }
    const scrolled = full ? 0 : Math.max(0, top - this.prevTop);
    return {
      full,
      cols: this.cols,
      rows: this.rows,
      alt: false,
      lines,
      cursor: { row: cursorRow, col: cursorCol, visible: true },
      title: this.title,
      scrollback: Math.min(top, MAX_LINES),
      mouse: false,
      scrolled,
    };
  }

  private remember(): void {
    if (this.alt) {
      this.prevRows = [];
      return;
    }
    const { rows, top } = this.layout();
    this.prevRows = rows.slice(top, top + this.rows).map((r) => JSON.stringify({ spans: toSpans(r.cells), wraps: r.wraps }));
    this.prevTop = top;
  }

  /** Something changed: send it within ~33 ms (at most 30 times a second). */
  private dirty(): void {
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      const full = this.needFull || !!this.alt;
      const screen = this.snapshot(full);
      this.needFull = false;
      this.remember();
      for (const emit of this.subs) emit(screen);
    }, 33);
  }

  dispose(): void {
    if (this.alt) clearInterval(this.alt.timer);
    if (this.timer) clearTimeout(this.timer);
    this.subs.clear();
  }
}
