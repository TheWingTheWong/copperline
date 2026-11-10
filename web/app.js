// Page logic: load a board, route it in a worker, animate the frames.
const EXAMPLES = [
  { name: "MCU breakout (QFP-48, 2 layers)", pcb: "examples/mcu48.kicad_pcb", pro: "examples/mcu48.kicad_pro" },
  { name: "QFN-24 fan-out (0.5 mm pitch)", pcb: "examples/qfn24.kicad_pcb", pro: "examples/qfn24.kicad_pro" },
];
const cv = document.getElementById("cv");
const ctx = cv.getContext("2d");
const statusEl = document.getElementById("status");
const statsEl = document.getElementById("stats");
const scrub = document.getElementById("scrub");
const replayBtn = document.getElementById("replay");
const dlBtn = document.getElementById("download");
const sel = document.getElementById("example");
const stage = document.getElementById("stage");
const LAYER = ["#d9483b", "#3b7dd8", "#e0a030", "#40a070"];
let worker = new Worker("worker.js");
let res = null, frame = 0, timer = null, fileName = "board";

EXAMPLES.forEach((e, i) => { const o = document.createElement("option"); o.value = i; o.textContent = e.name; sel.appendChild(o); });

function layerColor(l, nl) { return l === 0 ? LAYER[0] : l === nl - 1 ? LAYER[1] : LAYER[2 + (l % 2)]; }

function shapePath(s) {
  const p = new Path2D();
  if (s.k === "c") p.arc(s.p[0][0], s.p[0][1], Math.max(s.r, 0.01), 0, 2 * Math.PI);
  else if (s.k === "p") { s.p.forEach((q, i) => (i ? p.lineTo(q[0], q[1]) : p.moveTo(q[0], q[1]))); p.closePath(); }
  return p;
}
function fillShape(s, color) {
  if (s.k === "s") {
    ctx.strokeStyle = color; ctx.lineWidth = 2 * s.r; ctx.lineCap = "round";
    ctx.beginPath(); ctx.moveTo(s.p[0][0], s.p[0][1]); ctx.lineTo(s.p[1][0], s.p[1][1]); ctx.stroke();
    return;
  }
  ctx.fillStyle = color; const path = shapePath(s); ctx.fill(path);
  if (s.k === "p" && s.r > 0) { ctx.strokeStyle = color; ctx.lineWidth = 2 * s.r; ctx.lineJoin = "round"; ctx.stroke(path); }
}

function setup() {
  const [x0, y0, x1, y1] = res.bbox, pad = 1.5;
  const w = x1 - x0 + 2 * pad, h = y1 - y0 + 2 * pad;
  const maxW = stage.clientWidth - 8, maxH = Math.max(420, window.innerHeight - 140);
  const scale = Math.min(maxW / w, maxH / h);
  const dpr = window.devicePixelRatio || 1;
  cv.width = Math.round(w * scale * dpr); cv.height = Math.round(h * scale * dpr);
  cv.style.width = `${Math.round(w * scale)}px`; cv.style.height = `${Math.round(h * scale)}px`;
  res.tf = [scale * dpr, x0 - pad, y0 - pad];
}

function draw(fi) {
  const f = res.frames[fi], nl = res.layers.length, [k, ox, oy] = res.tf;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.fillStyle = "#10151c"; ctx.fillRect(0, 0, cv.width, cv.height);
  ctx.setTransform(k, 0, 0, k, -ox * k, -oy * k);
  ctx.strokeStyle = "#c8c26a"; ctx.lineWidth = 0.15; ctx.beginPath();
  for (const [a, b, c, d] of res.edges) { ctx.moveTo(a, b); ctx.lineTo(c, d); }
  ctx.stroke();
  const hot = new Set(f.hot);
  for (let l = nl - 1; l >= 0; l--) {
    ctx.globalAlpha = 0.9;
    for (const fx of res.fixed) if (fx.l === l) fillShape(fx.s, layerColor(l, nl));
    ctx.lineCap = "round";
    for (const t of f.t) {
      if (t[0] !== l) continue;
      ctx.strokeStyle = hot.has(t[6]) ? "#f4f4f4" : layerColor(l, nl);
      ctx.lineWidth = t[5];
      ctx.beginPath(); ctx.moveTo(t[1], t[2]); ctx.lineTo(t[3], t[4]); ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
  for (const p of res.pads) {
    for (const s of p.s) fillShape(s, p.top ? "#e8b04a" : "#7aa7e0");
    if (p.h) fillShape(p.h, "#10151c");
  }
  for (const [x, y, d, dr] of f.v) {
    ctx.fillStyle = "#cfcfcf"; ctx.beginPath(); ctx.arc(x, y, d / 2, 0, 2 * Math.PI); ctx.fill();
    ctx.fillStyle = "#10151c"; ctx.beginPath(); ctx.arc(x, y, dr / 2, 0, 2 * Math.PI); ctx.fill();
  }
  scrub.value = fi;
  const last = fi === res.frames.length - 1;
  statusEl.textContent = last
    ? `Done: ${res.stats.connections_routed}/${res.stats.connections} connections in ${res.wallMs.toFixed(0)} ms.`
    : f.it === 0 ? `First pass: nets route freely through each other (${fi + 1}/${res.frames.length}).`
    : `Negotiation pass ${f.it}: ${f.conflicts} conflicting cells, ${f.hot.length} nets in conflict.`;
}

function play() {
  clearInterval(timer);
  frame = 0;
  timer = setInterval(() => {
    draw(frame);
    if (++frame >= res.frames.length) clearInterval(timer);
  }, Math.max(40, Math.min(250, 4000 / res.frames.length)));
}

function showStats() {
  const s = res.stats;
  const rows = [["Nets", `${s.nets_complete}/${s.nets}`], ["Connections", `${s.connections_routed}/${s.connections}`],
    ["Negotiation passes", s.iterations], ["Tracks", s.tracks], ["Vias", s.vias],
    ["Track length", `${s.length_mm.toFixed(0)} mm`], ["Grid pitch", `${s.pitch.toFixed(3)} mm`], ["Route time", `${res.wallMs.toFixed(0)} ms`]];
  statsEl.innerHTML = rows.map(([a, b]) => `<tr><td>${a}</td><td>${b}</td></tr>`).join("");
}

function route(pcb, pro, name) {
  fileName = name; clearInterval(timer);
  statusEl.textContent = "Routing..."; replayBtn.disabled = dlBtn.disabled = scrub.disabled = true;
  worker.onmessage = (e) => {
    if (e.data.error) { statusEl.textContent = `Error: ${e.data.error}`; return; }
    res = e.data.result; res.wallMs = e.data.ms;
    setup(); showStats();
    scrub.max = res.frames.length - 1;
    replayBtn.disabled = dlBtn.disabled = scrub.disabled = false;
    play();
  };
  worker.postMessage({ pcb, pro });
}

async function loadExample(i) {
  const e = EXAMPLES[i];
  const [pcb, pro] = await Promise.all([fetch(e.pcb).then((r) => r.text()), fetch(e.pro).then((r) => r.text())]);
  route(pcb, pro, e.pcb.split("/").pop().replace(".kicad_pcb", ""));
}

async function openFiles(files) {
  const list = [...files];
  const pcbF = list.find((f) => f.name.endsWith(".kicad_pcb"));
  if (!pcbF) { statusEl.textContent = "Pick a .kicad_pcb file (KiCad 7-9 format)."; return; }
  const proF = list.find((f) => f.name.endsWith(".kicad_pro"));
  route(await pcbF.text(), proF ? await proF.text() : "", pcbF.name.replace(".kicad_pcb", ""));
}

sel.onchange = () => loadExample(sel.value);
document.getElementById("file").onchange = (e) => openFiles(e.target.files);
replayBtn.onclick = play;
scrub.oninput = () => { clearInterval(timer); draw(+scrub.value); };
dlBtn.onclick = () => {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([res.routed], { type: "text/plain" }));
  a.download = `${fileName}-routed.kicad_pcb`; a.click();
};
stage.ondragover = (e) => { e.preventDefault(); stage.classList.add("over"); };
stage.ondragleave = () => stage.classList.remove("over");
stage.ondrop = (e) => { e.preventDefault(); stage.classList.remove("over"); openFiles(e.dataTransfer.files); };
window.onresize = () => { if (res) { setup(); draw(Math.min(frame, res.frames.length - 1)); } };
const ex = +(new URLSearchParams(location.search).get("ex") || 0);
sel.value = ex;
loadExample(ex);
