/**
 * Model YAML that keeps its file's text on save. A save edits the source text
 * only where the model changed: a changed value is replaced in place, an added
 * key or list item is inserted at the indentation around it, and a removed one
 * is cut out. Comments, blank lines, key order, flow lists, alignment and
 * number formatting everywhere else stay as they were. The result is parsed
 * again and must equal the model; if it doesn't, the file is written fresh.
 *
 * `!zone <name>` (a surface's interzone boundary) reads as `{ zone: name }`
 * and is written back with its tag.
 */
import { Document, isMap, isScalar, isSeq, parseDocument, Scalar, YAMLMap, YAMLSeq } from "yaml";
import type { Node, Pair, ScalarTag } from "yaml";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Model = Record<string, any>;

const zoneTag: ScalarTag = {
  tag: "!zone",
  identify: () => false,
  resolve: (str: string) => ({ zone: str }),
};

const PARSE: { customTags: ScalarTag[] } = { customTags: [zoneTag] };

export interface LoadedModel {
  /** The file's text, which a save edits. */
  source: string;
  model: Model;
}

export function parseModel(text: string): LoadedModel {
  return { source: text, model: toModel(text) };
}

function toModel(text: string): Model {
  const doc = parseDocument(text, PARSE);
  if (doc.errors.length > 0) throw new Error(doc.errors[0].message);
  const js = doc.toJS({ maxAliasCount: -1 });
  return isPlainObject(js) ? (js as Model) : {};
}

/**
 * The text for `next`: `source` (whose model is `base`) edited where they
 * differ, or a fresh file when there is no source or the edit can't be made.
 */
export function serializeModel(source: string | null, base: Model | null, next: Model): string {
  if (source !== null && base !== null) {
    try {
      const doc = parseDocument(source, PARSE);
      if (doc.errors.length === 0 && isMap(doc.contents)) {
        const ed = new Editor(source);
        ed.map(doc.contents, base, next);
        const text = ed.apply();
        if (deepEqual(toModel(text), stripUndefined(next))) return text;
      }
    } catch {
      // Fall through to a fresh file.
    }
  }
  return freshText(next, source === null ? true : indentsSeq(source));
}

// ─── Values ──────────────────────────────────────────────────────────────────

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function stripUndefined(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(stripUndefined);
  if (isPlainObject(v)) {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) if (x !== undefined) out[k] = stripUndefined(x);
    return out;
  }
  return v;
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((x, i) => deepEqual(x, b[i]));
  }
  if (isPlainObject(a) && isPlainObject(b)) {
    const ka = Object.keys(a).filter((k) => a[k] !== undefined);
    const kb = Object.keys(b).filter((k) => b[k] !== undefined);
    return ka.length === kb.length && ka.every((k) => deepEqual(a[k], b[k]));
  }
  return false;
}

function isZoneBoundary(key: string | null, v: unknown): v is { zone: string } {
  return key === "boundary" && isPlainObject(v) && Object.keys(v).length === 1 && typeof v.zone === "string";
}

/** A node for `value`; under a `boundary` key, `{ zone }` becomes `!zone`. */
function makeNode(doc: Document, value: unknown, key: string | null): Node {
  if (isZoneBoundary(key, value)) {
    const s = new Scalar(value.zone);
    s.tag = "!zone";
    return s;
  }
  if (Array.isArray(value)) {
    const seq = new YAMLSeq();
    for (const item of value) seq.items.push(makeNode(doc, item, null));
    return seq;
  }
  if (isPlainObject(value)) {
    const map = new YAMLMap();
    for (const [k, v] of Object.entries(value)) {
      if (v !== undefined) map.items.push(doc.createPair(k, makeNode(doc, v, k)) as Pair<unknown, unknown>);
    }
    return map;
  }
  return doc.createNode(value) as Node;
}

/** Block YAML for `value` (a map entry when `key` is given), no indentation. */
function blockText(value: unknown, key: string | null, indentSeq: boolean): string {
  const doc = new Document(undefined, PARSE);
  const node = makeNode(doc, value, key);
  if (key !== null) {
    const map = new YAMLMap();
    map.items.push(doc.createPair(key, node) as Pair<unknown, unknown>);
    doc.contents = map;
  } else {
    doc.contents = node as YAMLMap;
  }
  return doc.toString({ lineWidth: 0, indentSeq }).replace(/\n$/, "");
}

function freshText(model: Model, indentSeq: boolean): string {
  const doc = new Document(undefined, PARSE);
  doc.contents = makeNode(doc, stripUndefined(model), null) as YAMLMap;
  return doc.toString({ lineWidth: 0, indentSeq });
}

/** Whether the file indents a block list under its key (`key:\n  - a`). */
function indentsSeq(source: string): boolean {
  const m = source.match(/^( *)[^\s#-][^\n]*:[ \t]*(?:#[^\n]*)?\n( *)- /m);
  return m ? m[2].length > m[1].length : true;
}

// ─── Text edits ──────────────────────────────────────────────────────────────

interface Edit {
  start: number;
  end: number;
  text: string;
}

class Editor {
  private edits: Edit[] = [];
  private readonly src: string;
  private readonly indentSeq: boolean;
  constructor(src: string) {
    this.src = src;
    this.indentSeq = indentsSeq(src);
  }

  apply(): string {
    const sorted = [...this.edits].sort((a, b) => b.start - a.start || b.end - a.end);
    let out = this.src;
    let floor = Infinity;
    for (const e of sorted) {
      if (e.end > floor) throw new Error("overlapping edits");
      out = out.slice(0, e.start) + e.text + out.slice(e.end);
      floor = e.start;
    }
    return out;
  }

  private lineStart(pos: number): number {
    return this.src.lastIndexOf("\n", pos - 1) + 1;
  }

  /** Index just past the line holding `pos` (after its newline). */
  private nextLine(pos: number): number {
    const nl = this.src.indexOf("\n", pos);
    return nl < 0 ? this.src.length : nl + 1;
  }

  private column(pos: number): number {
    return pos - this.lineStart(pos);
  }

  /** Whether only spaces precede `pos` on its line. */
  private startsLine(pos: number): boolean {
    return this.src.slice(this.lineStart(pos), pos).trim() === "";
  }

  private indent(text: string, col: number): string {
    const pad = " ".repeat(col);
    return text
      .split("\n")
      .map((l) => (l.length > 0 ? pad + l : l))
      .join("\n");
  }

  private push(start: number, end: number, text: string) {
    this.edits.push({ start, end, text });
  }

  /** The source text of a scalar for `value`, keeping `old`'s style. */
  private scalarText(old: Scalar | null, value: unknown): string {
    if (old && typeof value === "number" && typeof old.value === "number" && Number.isFinite(value)) {
      const src = this.src.slice(old.range![0], old.range![1]);
      if (Number.isInteger(value) && /^[-+]?\d+\.\d*$/.test(src)) return `${value}.0`;
      return String(value);
    }
    if (old && typeof value === "string") {
      if (old.type === Scalar.QUOTE_DOUBLE) return JSON.stringify(value);
      if (old.type === Scalar.QUOTE_SINGLE) return `'${value.replace(/'/g, "''")}'`;
    }
    const doc = new Document(undefined, PARSE);
    doc.contents = doc.createNode(value) as Scalar;
    return doc.toString({ lineWidth: 0 }).replace(/\n$/, "");
  }

  /** Replace a whole node with text for `value`. */
  private replaceNode(node: Node, value: unknown, key: string | null) {
    const [start, end] = [node.range![0], node.range![1]];
    const flow = (isMap(node) || isSeq(node)) && node.flow;
    if (isZoneBoundary(key, value)) {
      // A tagged scalar's range starts after its tag.
      const tagged = isScalar(node) && node.tag === "!zone";
      this.push(start, end, `${tagged ? "" : "!zone "}${this.scalarText(null, value.zone)}`);
    } else if (!isPlainObject(value) && !Array.isArray(value)) {
      this.push(start, end, this.scalarText(isScalar(node) && node.tag !== "!zone" ? node : null, value));
    } else if (flow || (Array.isArray(value) && value.length === 0) || (isPlainObject(value) && Object.keys(value).length === 0)) {
      const doc = new Document(undefined, PARSE);
      const made = makeNode(doc, value, key);
      if (isMap(made) || isSeq(made)) made.flow = true;
      doc.contents = made as YAMLMap;
      this.push(start, end, doc.toString({ lineWidth: 0 }).replace(/\n$/, ""));
    } else {
      throw new Error("block replacement");
    }
  }

  /** Edit `node` (whose value is `base`) to hold `next`. */
  private node(node: Node | null, base: unknown, next: unknown, key: string | null, owner: Pair<unknown, unknown> | null) {
    if (!node || !node.range) throw new Error("node without source");
    if (isMap(node) && node.tag !== "!zone" && isPlainObject(base) && isPlainObject(next) && !isZoneBoundary(key, next)) {
      this.map(node, base, next);
    } else if (isSeq(node) && Array.isArray(base) && Array.isArray(next)) {
      this.seq(node, base, next);
    } else {
      try {
        this.replaceNode(node, next, key);
      } catch (e) {
        // A block collection in place of something else: rewrite the entry.
        if (!owner || key === null) throw e;
        this.replacePair(owner, key, next);
      }
    }
  }

  /** Rewrite a whole `key: value` entry of a block map. */
  private replacePair(pair: Pair<unknown, unknown>, key: string, next: unknown) {
    const k = pair.key as Node;
    const v = pair.value as Node | null;
    const start = k.range![0];
    if (!this.startsLine(start)) throw new Error("entry shares a line");
    const end = v?.range ? v.range[1] : k.range![1];
    const text = blockText(next, key, this.indentSeq);
    this.push(start, end, this.indent(text, this.column(start)).trimStart());
  }

  map(map: YAMLMap, base: Record<string, unknown>, next: Record<string, unknown>) {
    if (map.flow || Object.values(next).every((v) => v === undefined)) {
      if (!deepEqual(base, next)) this.replaceNode(map, next, null);
      return;
    }
    const pairs = map.items as Pair<unknown, unknown>[];
    const keyOf = (p: Pair<unknown, unknown>) => (isScalar(p.key) ? String(p.key.value) : String(p.key));
    let lastKept: Pair<unknown, unknown> | null = null;
    for (const pair of pairs) {
      const k = keyOf(pair);
      if (!(k in next) || next[k] === undefined) {
        const kn = pair.key as Node;
        if (!this.startsLine(kn.range![0])) throw new Error("removing an entry that shares a line");
        const v = pair.value as Node | null;
        const end = v?.range ? v.range[1] : kn.range![1];
        this.push(this.lineStart(kn.range![0]), this.nextLine(Math.max(end - 1, kn.range![0])), "");
        continue;
      }
      lastKept = pair;
      if (deepEqual(base[k], next[k])) continue;
      if (pair.value === null || pair.value === undefined) {
        this.replacePair(pair, k, next[k]);
      } else {
        this.node(pair.value as Node, base[k], next[k], k, pair);
      }
    }
    const added = Object.entries(next).filter(([k, v]) => v !== undefined && !pairs.some((p) => keyOf(p) === k));
    if (added.length === 0) return;
    const anchor = lastKept ?? pairs[pairs.length - 1];
    if (!anchor) throw new Error("adding to an empty map");
    const ak = anchor.key as Node;
    const col = this.column(ak.range![0]);
    const av = anchor.value as Node | null;
    const end = av?.range ? Math.max(av.range[1] - 1, ak.range![0]) : ak.range![0];
    const at = this.nextLine(end);
    const text = added.map(([k, v]) => this.indent(blockText(v, k, this.indentSeq), col)).join("\n");
    const lead = at === this.src.length && !this.src.endsWith("\n") ? "\n" : "";
    this.push(at, at, `${lead}${text}\n`);
  }

  private seq(seq: YAMLSeq, base: unknown[], next: unknown[]) {
    if (seq.flow || next.length === 0) {
      if (!deepEqual(base, next)) this.replaceNode(seq, next, null);
      return;
    }
    const nodes = seq.items as Node[];
    if (nodes.length !== base.length) throw new Error("sequence out of step");
    if (base.length === next.length) {
      next.forEach((v, i) => {
        if (!deepEqual(base[i], v)) this.node(nodes[i], base[i], v, null, null);
      });
      return;
    }
    if (nodes.length === 0) throw new Error("adding to an empty block list");
    // Keep items equal to an old one (longest common subsequence), cut the
    // rest of the old ones and insert the new ones where they fall.
    const n = base.length;
    const m = next.length;
    const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lcs[i][j] = deepEqual(base[i], next[j]) ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
      }
    }
    const dash = (node: Node) => this.src.lastIndexOf("-", node.range![0]);
    const col = this.column(dash(nodes[0]));
    const itemEnd = (node: Node) => this.nextLine(Math.max(node.range![1] - 1, node.range![0]));
    const insertAt = (i: number) => (i < n ? this.lineStart(dash(nodes[i])) : itemEnd(nodes[n - 1]));
    let i = 0;
    let j = 0;
    let pending: string[] = [];
    const flush = () => {
      if (pending.length === 0) return;
      const at = insertAt(i);
      const lead = at === this.src.length && !this.src.endsWith("\n") ? "\n" : "";
      this.push(at, at, `${lead}${pending.join("\n")}\n`);
      pending = [];
    };
    while (i < n || j < m) {
      if (i < n && j < m && deepEqual(base[i], next[j])) {
        flush();
        i++;
        j++;
      } else if (i < n && (j >= m || lcs[i + 1][j] >= lcs[i][j + 1])) {
        flush();
        const d = dash(nodes[i]);
        if (!this.startsLine(d)) throw new Error("list item shares a line");
        this.push(this.lineStart(d), itemEnd(nodes[i]), "");
        i++;
      } else {
        pending.push(this.indent(blockText([next[j]], null, this.indentSeq), col));
        j++;
      }
    }
    flush();
  }
}
