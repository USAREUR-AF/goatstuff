// SPDX-License-Identifier: Apache-2.0
//
// jq-lite — a deliberately small subset of jq for the panoscope side-panel
// Decoded stream toolkit. Covers the inspection patterns operators reach for
// 80% of the time without the ~500 KiB wasm-jq dependency.
//
// Supported syntax:
//
//   .                        → identity (the whole sample)
//   .field                   → object index by key
//   .field.nested            → chained object index
//   .field[0]                → numeric array index
//   .field[]                 → iterate array; each element becomes a separate
//                              output value
//   .arr[].field             → iterate + index
//   select(.k == "v")        → predicate; keep the input iff the comparison
//                              holds (== / != / < / <= / > / >=)
//   <expr> | <expr>          → pipe; left output feeds right
//
// Each compiled filter is a function `(input: JsonValue) => JsonValue[]` —
// outputting an array because a single jq expression can yield 0..N values
// (filtered out, single, iterated). The frontend renders the array as
// successive sample rows under one timestamp.
//
// Not supported (by design — out of scope for the toolkit):
//   - constructors `{...}` / `[...]`
//   - operators `+` / `-` / `*` / `/` / `%`
//   - functions other than `select`
//   - variable bindings `as $x`
//   - try/catch, alternative `//`
//
// The parser is a recursive-descent over a hand-rolled tokenizer. ~280 LOC
// total including tests. Operator-grade error messages — the input box's
// error banner shows the literal parser message + the column where parsing
// stalled.

export type JsonValue = null | boolean | number | string | JsonValue[] | { [k: string]: JsonValue };

export type CompiledFilter = (input: JsonValue) => JsonValue[];

export interface CompileError {
  message: string;
  col: number;
}

export interface CompileOk {
  filter: CompiledFilter;
}

export type CompileResult = CompileOk | { error: CompileError };

/** Compile `src` into a filter or return a structured error. Empty / whitespace
 *  source compiles to identity. */
export function compileFilter(src: string): CompileResult {
  const trimmed = src.trim();
  if (trimmed.length === 0) {
    return { filter: (v) => [v] };
  }
  try {
    const tokens = tokenize(src);
    const parser = new Parser(tokens, src);
    const expr = parser.parsePipe();
    if (parser.peek().type !== "eof") {
      throw new ParseError(`unexpected token ${formatTok(parser.peek())}`, parser.peek().col);
    }
    return { filter: (v) => evalExpr(expr, v) };
  } catch (e) {
    if (e instanceof ParseError) {
      return { error: { message: e.message, col: e.col } };
    }
    return { error: { message: (e as Error).message ?? String(e), col: 0 } };
  }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

type Token =
  | { type: "dot"; col: number }
  | { type: "lbracket"; col: number }
  | { type: "rbracket"; col: number }
  | { type: "lparen"; col: number }
  | { type: "rparen"; col: number }
  | { type: "pipe"; col: number }
  | { type: "ident"; value: string; col: number }
  | { type: "number"; value: number; col: number }
  | { type: "string"; value: string; col: number }
  | { type: "op"; value: "==" | "!=" | "<" | "<=" | ">" | ">="; col: number }
  | { type: "eof"; col: number };

function tokenize(src: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const peekCh = (off = 0) => src[i + off];
  while (i < src.length) {
    const ch = src[i];
    if (ch === " " || ch === "\t" || ch === "\n") {
      i++;
      continue;
    }
    const col = i;
    if (ch === ".") { tokens.push({ type: "dot", col }); i++; continue; }
    if (ch === "[") { tokens.push({ type: "lbracket", col }); i++; continue; }
    if (ch === "]") { tokens.push({ type: "rbracket", col }); i++; continue; }
    if (ch === "(") { tokens.push({ type: "lparen", col }); i++; continue; }
    if (ch === ")") { tokens.push({ type: "rparen", col }); i++; continue; }
    if (ch === "|") { tokens.push({ type: "pipe", col }); i++; continue; }
    if (ch === "=" && peekCh(1) === "=") { tokens.push({ type: "op", value: "==", col }); i += 2; continue; }
    if (ch === "!" && peekCh(1) === "=") { tokens.push({ type: "op", value: "!=", col }); i += 2; continue; }
    if (ch === "<" && peekCh(1) === "=") { tokens.push({ type: "op", value: "<=", col }); i += 2; continue; }
    if (ch === ">" && peekCh(1) === "=") { tokens.push({ type: "op", value: ">=", col }); i += 2; continue; }
    if (ch === "<") { tokens.push({ type: "op", value: "<", col }); i++; continue; }
    if (ch === ">") { tokens.push({ type: "op", value: ">", col }); i++; continue; }
    if (ch === '"' || ch === "'") {
      const quote = ch;
      let s = "";
      i++;
      while (i < src.length && src[i] !== quote) {
        if (src[i] === "\\" && i + 1 < src.length) {
          const esc = src[i + 1];
          s += esc === "n" ? "\n" : esc === "t" ? "\t" : esc;
          i += 2;
        } else {
          s += src[i++];
        }
      }
      if (i >= src.length) throw new ParseError(`unterminated string`, col);
      i++; // closing quote
      tokens.push({ type: "string", value: s, col });
      continue;
    }
    if ((ch >= "0" && ch <= "9") || (ch === "-" && peekCh(1) >= "0" && peekCh(1) <= "9")) {
      let j = i;
      if (src[j] === "-") j++;
      while (j < src.length && ((src[j] >= "0" && src[j] <= "9") || src[j] === ".")) j++;
      const num = Number(src.slice(i, j));
      if (Number.isNaN(num)) throw new ParseError(`invalid number`, col);
      tokens.push({ type: "number", value: num, col });
      i = j;
      continue;
    }
    if ((ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z") || ch === "_") {
      let j = i;
      while (
        j < src.length &&
        ((src[j] >= "a" && src[j] <= "z") ||
          (src[j] >= "A" && src[j] <= "Z") ||
          (src[j] >= "0" && src[j] <= "9") ||
          src[j] === "_")
      ) j++;
      tokens.push({ type: "ident", value: src.slice(i, j), col });
      i = j;
      continue;
    }
    throw new ParseError(`unexpected character '${ch}'`, col);
  }
  tokens.push({ type: "eof", col: src.length });
  return tokens;
}

// ---------------------------------------------------------------------------
// Parser + AST
// ---------------------------------------------------------------------------

type Expr =
  | { kind: "identity" }
  | { kind: "field"; name: string; from: Expr }
  | { kind: "index"; idx: number; from: Expr }
  | { kind: "iterate"; from: Expr }
  | { kind: "pipe"; left: Expr; right: Expr }
  | { kind: "select"; predicate: Expr }
  | { kind: "compare"; op: "==" | "!=" | "<" | "<=" | ">" | ">="; left: Expr; right: Expr }
  | { kind: "literal"; value: JsonValue };

class ParseError extends Error {
  constructor(message: string, public col: number) { super(message); }
}

function formatTok(t: Token): string {
  if (t.type === "eof") return "<end>";
  if (t.type === "ident" || t.type === "string") return `'${t.value}'`;
  if (t.type === "number") return String(t.value);
  if (t.type === "op") return `'${t.value}'`;
  return `'${t.type}'`;
}

class Parser {
  pos = 0;
  constructor(public tokens: Token[], public src: string) {}
  peek(off = 0): Token { return this.tokens[this.pos + off]; }
  next(): Token { return this.tokens[this.pos++]; }
  eat(type: Token["type"]): Token {
    const t = this.peek();
    if (t.type !== type) throw new ParseError(`expected ${type}, got ${formatTok(t)}`, t.col);
    return this.next();
  }

  parsePipe(): Expr {
    let left = this.parseComparison();
    while (this.peek().type === "pipe") {
      this.next();
      const right = this.parseComparison();
      left = { kind: "pipe", left, right };
    }
    return left;
  }

  parseComparison(): Expr {
    const left = this.parseTerm();
    if (this.peek().type === "op") {
      const op = this.next() as Extract<Token, { type: "op" }>;
      const right = this.parseTerm();
      return { kind: "compare", op: op.value, left, right };
    }
    return left;
  }

  parseTerm(): Expr {
    const t = this.peek();
    if (t.type === "ident" && t.value === "select") {
      this.next();
      this.eat("lparen");
      const predicate = this.parsePipe();
      this.eat("rparen");
      return { kind: "select", predicate };
    }
    if (t.type === "string") { this.next(); return { kind: "literal", value: t.value }; }
    if (t.type === "number") { this.next(); return { kind: "literal", value: t.value }; }
    if (t.type === "ident" && (t.value === "true" || t.value === "false")) {
      this.next();
      return { kind: "literal", value: t.value === "true" };
    }
    if (t.type === "ident" && t.value === "null") {
      this.next();
      return { kind: "literal", value: null };
    }
    if (t.type === "lparen") {
      this.next();
      const inner = this.parsePipe();
      this.eat("rparen");
      return this.parsePath(inner);
    }
    if (t.type === "dot") {
      return this.parsePath({ kind: "identity" });
    }
    throw new ParseError(`expected expression, got ${formatTok(t)}`, t.col);
  }

  parsePath(from: Expr): Expr {
    let cur = from;
    // Leading dot may be either a standalone identity (`.`) or the start of
    // a field/index chain (`.foo`, `.[0]`, etc.).
    if (this.peek().type === "dot") {
      this.next();
    }
    while (true) {
      const t = this.peek();
      if (t.type === "ident") {
        this.next();
        cur = { kind: "field", name: t.value, from: cur };
      } else if (t.type === "lbracket") {
        this.next();
        const inner = this.peek();
        if (inner.type === "rbracket") {
          this.next();
          cur = { kind: "iterate", from: cur };
        } else if (inner.type === "number") {
          this.next();
          this.eat("rbracket");
          cur = { kind: "index", idx: inner.value, from: cur };
        } else {
          throw new ParseError(`expected number or ] inside [], got ${formatTok(inner)}`, inner.col);
        }
      } else if (t.type === "dot") {
        this.next();
      } else {
        return cur;
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

function evalExpr(expr: Expr, input: JsonValue): JsonValue[] {
  switch (expr.kind) {
    case "identity": return [input];
    case "literal": return [expr.value];
    case "field": {
      const ins = evalExpr(expr.from, input);
      const out: JsonValue[] = [];
      for (const v of ins) {
        if (v !== null && typeof v === "object" && !Array.isArray(v)) {
          out.push(expr.name in v ? v[expr.name] : null);
        } else {
          out.push(null);
        }
      }
      return out;
    }
    case "index": {
      const ins = evalExpr(expr.from, input);
      const out: JsonValue[] = [];
      for (const v of ins) {
        if (Array.isArray(v)) {
          const i = expr.idx < 0 ? v.length + expr.idx : expr.idx;
          out.push(i >= 0 && i < v.length ? v[i] : null);
        } else {
          out.push(null);
        }
      }
      return out;
    }
    case "iterate": {
      const ins = evalExpr(expr.from, input);
      const out: JsonValue[] = [];
      for (const v of ins) {
        if (Array.isArray(v)) out.push(...v);
        else if (v !== null && typeof v === "object") out.push(...Object.values(v));
        // scalars iterate to nothing — matches jq behavior closer to .[]?
      }
      return out;
    }
    case "pipe": {
      const lefts = evalExpr(expr.left, input);
      const out: JsonValue[] = [];
      for (const v of lefts) out.push(...evalExpr(expr.right, v));
      return out;
    }
    case "select": {
      const ps = evalExpr(expr.predicate, input);
      const truthy = ps.some((p) => isTruthy(p));
      return truthy ? [input] : [];
    }
    case "compare": {
      const lefts = evalExpr(expr.left, input);
      const rights = evalExpr(expr.right, input);
      const out: JsonValue[] = [];
      for (const l of lefts) {
        for (const r of rights) {
          out.push(applyCompare(expr.op, l, r));
        }
      }
      return out;
    }
  }
}

function isTruthy(v: JsonValue): boolean {
  return v !== null && v !== false;
}

function applyCompare(op: string, l: JsonValue, r: JsonValue): boolean {
  switch (op) {
    case "==": return jsonEqual(l, r);
    case "!=": return !jsonEqual(l, r);
    case "<": return jsonCompare(l, r) < 0;
    case "<=": return jsonCompare(l, r) <= 0;
    case ">": return jsonCompare(l, r) > 0;
    case ">=": return jsonCompare(l, r) >= 0;
    default: return false;
  }
}

function jsonEqual(a: JsonValue, b: JsonValue): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  if (typeof a !== typeof b) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((v, i) => jsonEqual(v, b[i]));
  }
  if (typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a as object);
    const kb = Object.keys(b as object);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => jsonEqual((a as Record<string, JsonValue>)[k], (b as Record<string, JsonValue>)[k]));
  }
  return false;
}

function jsonCompare(a: JsonValue, b: JsonValue): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "string" && typeof b === "string") return a < b ? -1 : a > b ? 1 : 0;
  // Cross-type comparisons return 0 — same conservative behavior as jq's
  // type-stripped paths, suitable for the inspection use case.
  return 0;
}
