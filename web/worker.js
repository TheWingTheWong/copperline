// Runs the router off the main thread. Messages: {pcb, pro} -> {result} | {error}.
let wasm = null;
const ready = (async () => {
  const resp = await fetch("copperline.wasm");
  const { instance } = await WebAssembly.instantiate(await resp.arrayBuffer(), {
    env: { copperline_now_ms: () => performance.now() },
  });
  wasm = instance.exports;
})();

function put(bytes) {
  const ptr = wasm.cl_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);
  return ptr;
}

onmessage = async (e) => {
  try {
    await ready;
    const enc = new TextEncoder();
    const pcb = enc.encode(e.data.pcb);
    const pro = enc.encode(e.data.pro || "");
    const a = put(pcb);
    const b = put(pro);
    const t0 = performance.now();
    const out = wasm.cl_route(a, pcb.length, b, pro.length);
    const ms = performance.now() - t0;
    const json = new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, out, wasm.cl_result_len()));
    wasm.cl_free(a, pcb.length);
    wasm.cl_free(b, pro.length);
    const result = JSON.parse(json);
    if (result.error) postMessage({ error: result.error });
    else postMessage({ result, ms });
  } catch (err) {
    postMessage({ error: String(err) });
  }
};
