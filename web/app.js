const $ = (id) => document.getElementById(id);
const ui = {
  start: $("start"),
  speed: $("speed"),
  unit: $("unit"),
  progress: $("progress"),
  phase: $("phase"),
  latency: $("latency"),
  jitter: $("jitter"),
  download: $("download"),
  upload: $("upload"),
  theme: $("theme"),
  reading: $("reading"),
  progressTrack: $("progress-track"),
  metrics: $("metrics"),
  resultActions: $("result-actions"),
  uploadMode: $("upload-mode"),
  copyResult: $("copy-result"),
  stages: {
    latency: $("stage-latency"),
    download: $("stage-download"),
    upload: $("stage-upload"),
  },
};
$("host").textContent = location.host;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const nonce = () => `${Date.now()}-${Math.random()}`;
const TEST_DURATION_MS = 5_000;
const DISPLAY_INTERVAL_MS = 100;
const TRANSFER_STREAMS = 4;
const RESULTS_KEY = "speedtest-results";
const MAX_SAVED_RESULTS = 500;
let currentResult = null;

function storedTheme() {
  try {
    return localStorage.getItem("theme");
  } catch {
    return null;
  }
}

function storeTheme(theme) {
  try {
    localStorage.setItem("theme", theme);
  } catch {
    // Persistence is optional; storage restrictions must not break the test.
  }
}

function saveResult(result) {
  try {
    const stored = JSON.parse(localStorage.getItem(RESULTS_KEY) || "[]");
    const results = Array.isArray(stored) ? stored : [];
    results.unshift(result);
    localStorage.setItem(
      RESULTS_KEY,
      JSON.stringify(results.slice(0, MAX_SAVED_RESULTS)),
    );
  } catch {
    // History is optional and remains local to this browser.
  }
}

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  ui.theme.setAttribute(
    "aria-label",
    `Switch to ${theme === "mocha" ? "Latte" : "Mocha"} theme`,
  );
  storeTheme(theme);
}
const savedTheme = storedTheme();
setTheme(
  savedTheme === "mocha" || savedTheme === "latte"
    ? savedTheme
    : (matchMedia("(prefers-color-scheme: dark)").matches ? "mocha" : "latte"),
);
ui.theme.addEventListener(
  "click",
  () =>
    setTheme(
      document.documentElement.dataset.theme === "mocha" ? "latte" : "mocha",
    ),
);
function show(value, label, progress, unit = "Mbps") {
  ui.speed.textContent = Number.isFinite(value)
    ? value.toFixed(value < 100 ? 1 : 0)
    : "—";
  ui.unit.textContent = unit;
  ui.phase.textContent = label;
  ui.progress.style.width = `${Math.min(100, progress)}%`;
}
function setStage(name, state) {
  ui.stages[name].className = state;
}
function resetStages() {
  Object.values(ui.stages).forEach((stage) => stage.className = "");
}
async function latency() {
  const samples = [];
  for (let i = 0; i < 10; i++) {
    const start = performance.now();
    await fetch(`/api/ping?n=${nonce()}`, { cache: "no-store" });
    samples.push(performance.now() - start);
    show(samples.at(-1), "Testing latency", (i + 1) / 10 * 100, "ms");
    await pause(45);
  }
  const sorted = [...samples].sort((a, b) => a - b);
  const jitter = samples.slice(1).reduce(
    (sum, value, index) => sum + Math.abs(value - samples[index]),
    0,
  ) / (samples.length - 1);
  return { median: sorted[Math.floor(sorted.length / 2)], jitter };
}
async function transferDown() {
  const requestSize = 512 * 1024 * 1024;
  const start = performance.now();
  const state = { bytes: 0, lastDisplay: 0 };
  show(0, "Testing download", 0);

  async function stream() {
    while (performance.now() - start < TEST_DURATION_MS) {
      const response = await fetch(
        `/api/download?size=${requestSize}&n=${nonce()}`,
        { cache: "no-store" },
      );
      if (!response.ok || !response.body) throw new Error("Download failed");
      const reader = response.body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        state.bytes += value.byteLength;
        const elapsed = performance.now() - start;
        updateTransferDisplay(state, elapsed, "Testing download");
        if (elapsed >= TEST_DURATION_MS) {
          await reader.cancel();
          break;
        }
      }
    }
  }

  await Promise.all(Array.from({ length: TRANSFER_STREAMS }, stream));
  return state.bytes * 8 / ((performance.now() - start) / 1000) / 1e6;
}

function supportsRequestStreams() {
  try {
    let duplexAccessed = false;
    const request = new Request(location.href, {
      method: "POST",
      body: new ReadableStream(),
      get duplex() {
        duplexAccessed = true;
        return "half";
      },
    });
    return duplexAccessed && !request.headers.has("Content-Type");
  } catch {
    return false;
  }
}

async function transferUp() {
  if (supportsRequestStreams()) {
    try {
      return { speed: await transferUpStreamed(), mode: "streamed" };
    } catch {
      // Safari, HTTP/1.x, or an incompatible proxy uses the portable fallback.
    }
  }
  return { speed: await transferUpBatched(), mode: "compatibility" };
}

async function transferUpStreamed() {
  const block = new Uint8Array(256 * 1024);
  const start = performance.now();
  const state = { bytes: 0, lastDisplay: 0 };
  show(0, "Testing upload", 0);

  async function stream() {
    const body = new ReadableStream({
      pull(controller) {
        const elapsed = performance.now() - start;
        if (elapsed >= TEST_DURATION_MS) {
          controller.close();
          return;
        }
        controller.enqueue(block);
        state.bytes += block.byteLength;
        updateTransferDisplay(state, elapsed, "Testing upload");
      },
    });
    const response = await fetch(`/api/upload?n=${nonce()}`, {
      method: "POST",
      headers: { "Content-Type": "application/octet-stream" },
      body,
      duplex: "half",
    });
    if (!response.ok) throw new Error("Streaming upload failed");
    return (await response.json()).bytes;
  }

  const totals = await Promise.all(
    Array.from({ length: TRANSFER_STREAMS }, stream),
  );
  const bytes = totals.reduce((sum, value) => sum + value, 0);
  return bytes * 8 / ((performance.now() - start) / 1000) / 1e6;
}

async function transferUpBatched() {
  const minimumChunk = 256 * 1024;
  const maximumChunk = 16 * 1024 * 1024;
  const start = performance.now();
  const state = { bytes: 0, lastDisplay: 0 };
  show(0, "Testing upload", 0);

  async function stream() {
    let chunkSize = minimumChunk;
    while (performance.now() - start < TEST_DURATION_MS) {
      const response = await fetch(`/api/upload?n=${nonce()}`, {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: new Uint8Array(chunkSize),
      });
      if (!response.ok) throw new Error("Upload failed");
      const result = await response.json();
      state.bytes += result.bytes;
      const elapsed = performance.now() - start;
      const seconds = elapsed / 1000;
      updateTransferDisplay(state, elapsed, "Testing upload");

      const bytesPerSecondPerStream = state.bytes / seconds / TRANSFER_STREAMS;
      const desired = bytesPerSecondPerStream * 0.5;
      chunkSize = Math.round(
        Math.min(maximumChunk, Math.max(minimumChunk, desired)) / minimumChunk,
      ) * minimumChunk;
    }
  }

  await Promise.all(Array.from({ length: TRANSFER_STREAMS }, stream));
  return state.bytes * 8 / ((performance.now() - start) / 1000) / 1e6;
}

function updateTransferDisplay(state, elapsed, label) {
  if (elapsed - state.lastDisplay < DISPLAY_INTERVAL_MS) return;
  show(
    state.bytes * 8 / (elapsed / 1000) / 1e6,
    label,
    elapsed / TEST_DURATION_MS * 100,
  );
  state.lastDisplay = elapsed;
}

function resultText(result) {
  const mode = result.uploadMode === "streamed" ? "streamed" : "compatibility";
  return [
    `Speedtest · ${new Date(result.timestamp).toLocaleString()}`,
    `Latency: ${result.latency.toFixed(1)} ms`,
    `Jitter: ${result.jitter.toFixed(1)} ms`,
    `Download: ${result.download.toFixed(result.download < 100 ? 1 : 0)} Mbps`,
    `Upload: ${
      result.upload.toFixed(result.upload < 100 ? 1 : 0)
    } Mbps (${mode})`,
    location.origin,
  ].join("\n");
}

async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
}

ui.copyResult.addEventListener("click", async () => {
  if (!currentResult) return;
  try {
    await copyText(resultText(currentResult));
    ui.copyResult.textContent = "Copied";
    setTimeout(() => ui.copyResult.textContent = "Copy result", 1_500);
  } catch {
    ui.copyResult.textContent = "Copy failed";
  }
});

ui.start.addEventListener("click", async () => {
  ui.reading.hidden = false;
  ui.progressTrack.hidden = false;
  ui.metrics.hidden = false;
  ui.resultActions.hidden = true;
  currentResult = null;
  ui.start.disabled = true;
  ui.start.querySelector("span").textContent = "Testing…";
  ui.latency.textContent =
    ui.jitter.textContent =
    ui.download.textContent =
    ui.upload.textContent =
      "—";
  resetStages();
  try {
    setStage("latency", "active");
    show(NaN, "Warming up", 0, "ms");
    await fetch(`/api/ping?n=${nonce()}`, { cache: "no-store" });
    const ping = await latency();
    ui.latency.textContent = ping.median.toFixed(1);
    ui.jitter.textContent = ping.jitter.toFixed(1);
    setStage("latency", "done");
    setStage("download", "active");
    const down = await transferDown();
    ui.download.textContent = down.toFixed(down < 100 ? 1 : 0);
    setStage("download", "done");
    setStage("upload", "active");
    await pause(250);
    const upload = await transferUp();
    const up = upload.speed;
    ui.upload.textContent = up.toFixed(up < 100 ? 1 : 0);
    setStage("upload", "done");
    currentResult = {
      timestamp: new Date().toISOString(),
      latency: ping.median,
      jitter: ping.jitter,
      download: down,
      upload: up,
      uploadMode: upload.mode,
    };
    saveResult(currentResult);
    ui.uploadMode.textContent = upload.mode === "streamed"
      ? "Streamed upload"
      : "Compatibility upload";
    ui.resultActions.hidden = false;
    show(NaN, "Test complete", 100, "");
  } catch (error) {
    show(NaN, error.message || "Test failed", 0, "");
  } finally {
    ui.start.disabled = false;
    ui.start.querySelector("span").textContent = "Test again";
  }
});
