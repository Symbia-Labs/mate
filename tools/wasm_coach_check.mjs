// Replay coach events (tools/game_events.py) through the browser build's coach exports, the way the Sparring page
// calls them, and write its lines in the shape of tools/quiet_stats.py's file, so the two can be compared.
//
// Usage: node tools/wasm_coach_check.mjs WASM EVENTS OUT [NATIVE_LINES.tsv]
//
// For a tutor event: coach_tutor (the Me line), then coach_ponder on the position after the move. For a student
// event: coach_student (the Game and You lines). With NATIVE_LINES, the quiet `say` rows of that file (from
// `uci coach voice=quiet`) are compared with these, row for row. Times are printed for each kind of call.
import { readFileSync, writeFileSync } from "node:fs";

const [wasmPath, eventsPath, outPath, nativePath] = process.argv.slice(2);
const MATE = 32000;
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const E = instance.exports;
const enc = new TextEncoder(), dec = new TextDecoder();
const put = s => { const b = enc.encode(s); const p = E.alloc(b.length); new Uint8Array(E.memory.buffer, p, b.length).set(b); return [p, b.length]; };
const out = () => dec.decode(new Uint8Array(E.memory.buffer, E.out_ptr(), E.out_len()));
const call = (fn, ...args) => { fn(...args); const r = out(); if (r.startsWith("!")) throw new Error(r); return r; };

const times = { student: [], tutor: [], ponder: [] };
const timed = (kind, f) => { const t = performance.now(); const r = f(); times[kind].push(performance.now() - t); return r; };
const rows = [];
let game = -1;
for (const line of readFileSync(eventsPath, "utf8").split("\n")) {
  if (!line) continue;
  const w = line.split(" ");
  if (w[0] === "G") { game = +w[1]; call(E.coach_new); continue; }
  const ply = +w[1], mv = w[2];
  let text;
  if (w[0] === "tutor") {
    const depth = +w[3], kind = w[4], value = +w[5], fen = w.slice(6).join(" ");
    const score = kind === "cp" ? value : value > 0 ? MATE - (2 * value - 1) : -MATE + 2 * -value;
    const [fp, fn] = put(fen), [mp, mn] = put(mv);
    text = timed("tutor", () => call(E.coach_tutor, ply, fp, fn, mp, mn, depth, score));
    const [fp2, fn2] = put(fen), [mp2, mn2] = put(mv);
    const after = call(E.play, fp2, fn2, mp2, mn2);
    const [ap, an] = put(after);
    timed("ponder", () => call(E.coach_ponder, ply, ap, an));
  } else {
    const fen = w.slice(3).join(" ");
    const [fp, fn] = put(fen), [mp, mn] = put(mv);
    text = timed("student", () => call(E.coach_student, ply, fp, fn, mp, mn));
  }
  for (const l of text.split("\n").filter(Boolean)) {
    const [lane, t] = l.split("\t");
    rows.push(`${game}\t${ply}\t${lane}\t${t}`);
  }
}
writeFileSync(outPath, rows.join("\n") + "\n");
const stat = a => { const s = [...a].sort((x, y) => x - y); return `median ${s[Math.floor(s.length / 2)].toFixed(0)} ms, 95th ${s[Math.floor(s.length * 0.95)].toFixed(0)} ms, longest ${s[s.length - 1].toFixed(0)} ms`; };
for (const k of Object.keys(times)) console.log(`${k}: ${times[k].length} calls, ${stat(times[k])}`);
console.log(`${rows.length} lines written to ${outPath}`);
if (nativePath) {
  const native = readFileSync(nativePath, "utf8").split("\n").filter(Boolean).map(l => l.split("\t"))
    .filter(r => r[0] === "quiet" && r[4] === "say").map(r => `${r[1]}\t${r[2]}\t${r[3]}\t${r[5]}`);
  const a = new Set(native), b = new Set(rows);
  const onlyNative = native.filter(r => !b.has(r)), onlyWasm = rows.filter(r => !a.has(r));
  console.log(`native ${native.length} lines, browser ${rows.length}; only native ${onlyNative.length}, only browser ${onlyWasm.length}`);
  for (const r of onlyNative.slice(0, 5)) console.log("  native only:", r);
  for (const r of onlyWasm.slice(0, 5)) console.log("  browser only:", r);
  process.exitCode = onlyNative.length || onlyWasm.length ? 1 : 0;
}
