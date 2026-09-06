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
  downloadUnit: $("download-unit"),
  upload: $("upload"),
  uploadUnit: $("upload-unit"),
  theme: $("theme"),
  reading: $("reading"),
  progressTrack: $("progress-track"),
  metrics: $("metrics"),
  resultChart: $("result-chart"),
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
const DURATION_MS = 5_000;
const DISPLAY_MS = 100;
const SAMPLE_MS = 250;
const STREAMS = 4;
const REQUEST_TIMEOUT_MS = 15_000;
const RESULTS_KEY = "results";
const MAX_RESULTS = 500;
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
      JSON.stringify(results.slice(0, MAX_RESULTS)),
    );
  } catch {
    // History is optional and remains local to this browser.
  }
}

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  ui.theme.setAttribute(
    "aria-label",
    i18n._("theme.switch", { theme: theme === "mocha" ? "Latte" : "Mocha" }),
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
  if (Number.isFinite(value) && unit === "Mbps") {
    const rate = i18n.rate(value);
    ui.speed.textContent = rate.number;
    ui.unit.textContent = rate.unit;
  } else {
    ui.speed.textContent = Number.isFinite(value)
      ? value.toFixed(value < 100 ? 1 : 0)
      : "—";
    ui.unit.textContent = unit;
  }
  ui.phase.textContent = label;
  ui.progress.style.width = `${Math.min(100, progress)}%`;
}
function setStage(name, state) {
  ui.stages[name].className = state;
}
function resetStages() {
  Object.values(ui.stages).forEach((stage) => stage.className = "");
}
// Abort and settle every peer before a failed stage can finish or fall back.
async function runStreams(stream, timeout = REQUEST_TIMEOUT_MS, allowTimeout = false) {
  const controller = new AbortController();
  let expired = false;
  const timer = setTimeout(() => {
    expired = true;
    controller.abort(new Error(i18n._("error.timeout")));
  }, timeout);
  const tasks = Array.from({ length: STREAMS }, () =>
    Promise.resolve().then(() => stream(controller.signal))
  );
  try {
    return await Promise.all(tasks);
  } catch (error) {
    if (!allowTimeout || !expired) throw error;
    return [];
  } finally {
    clearTimeout(timer);
    controller.abort();
    await Promise.allSettled(tasks);
  }
}

async function ping() {
  const controller = new AbortController();
  const timer = setTimeout(() => {
    controller.abort(new Error(i18n._("error.timeout")));
  }, REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch(`/api/ping?n=${nonce()}`, {
      cache: "no-store",
      signal: controller.signal,
    });
    if (!response.ok || (await response.json()).ok !== true) {
      throw new Error(i18n._("error.latency"));
    }
  } finally {
    clearTimeout(timer);
  }
}

async function latency() {
  const samples = [];
  for (let i = 0; i < 10; i++) {
    const start = performance.now();
    await ping();
    samples.push(performance.now() - start);
    show(samples.at(-1), i18n._("status.testingLatency"), (i + 1) / 10 * 100, "ms");
    await pause(45);
  }
  const sorted = [...samples].sort((a, b) => a - b);
  const jitter = samples.slice(1).reduce(
    (sum, value, index) => sum + Math.abs(value - samples[index]),
    0,
  ) / (samples.length - 1);
  return { median: (sorted[4] + sorted[5]) / 2, jitter };
}
async function transferDown() {
  const requestSize = 512 * 1024 * 1024;
  const start = performance.now();
  const state = transferState();
  show(0, i18n._("status.testingDownload"), 0);

  async function stream(signal) {
    while (performance.now() - start < DURATION_MS) {
      const response = await fetch(
        `/api/download?size=${requestSize}&n=${nonce()}`,
        { cache: "no-store", signal },
      );
      if (!response.ok || !response.body) {
        throw new Error(i18n._("error.download"));
      }
      const reader = response.body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done || signal.aborted) break;
        state.bytes += value.byteLength;
        const elapsed = performance.now() - start;
        updateTransfer(state, elapsed, i18n._("status.testingDownload"));
        if (elapsed >= DURATION_MS) {
          await reader.cancel();
          break;
        }
      }
    }
  }

  await runStreams(stream, DURATION_MS, true);
  if (state.bytes === 0) throw new Error(i18n._("error.download"));
  return finishTransfer(state, start);
}

function canStreamUpload() {
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
  if (canStreamUpload()) {
    try {
      return { ...await transferUpStreamed(), mode: "streamed" };
    } catch {
      // Safari, HTTP/1.x, or an incompatible proxy uses the portable fallback.
    }
  }
  return { ...await transferUpBatched(), mode: "compatibility" };
}

async function transferUpStreamed() {
  const block = new Uint8Array(256 * 1024);
  const start = performance.now();
  const state = transferState();
  show(0, i18n._("status.testingUpload"), 0);

  async function stream(signal) {
    const body = new ReadableStream({
      pull(controller) {
        signal.throwIfAborted();
        const elapsed = performance.now() - start;
        if (elapsed >= DURATION_MS) {
          controller.close();
          return;
        }
        controller.enqueue(block);
        state.bytes += block.byteLength;
        updateTransfer(state, elapsed, i18n._("status.testingUpload"));
      },
    });
    const response = await fetch(`/api/upload?n=${nonce()}`, {
      method: "POST",
      signal,
      headers: { "Content-Type": "application/octet-stream" },
      body,
      duplex: "half",
    });
    if (!response.ok) throw new Error(i18n._("error.streamingUpload"));
    return (await response.json()).bytes;
  }

  const totals = await runStreams(stream);
  const bytes = totals.reduce((sum, value) => sum + value, 0);
  return finishTransfer(state, start, bytes);
}

function uploadBatch(size, signal, onProgress) {
  return new Promise((resolve, reject) => {
    signal.throwIfAborted();
    const request = new XMLHttpRequest();
    let reported = 0;
    const progress = (loaded) => {
      const bytes = Math.max(reported, Math.min(size, loaded));
      onProgress(bytes - reported);
      reported = bytes;
    };
    const cleanup = () => {
      signal.removeEventListener("abort", abort);
      request.onload = request.onerror = request.onabort = null;
      request.upload.onprogress = null;
    };
    const fail = (error) => {
      cleanup();
      reject(error);
    };
    const abort = () => {
      request.abort();
      fail(signal.reason);
    };
    request.open("POST", `/api/upload?n=${nonce()}`);
    request.setRequestHeader("Content-Type", "application/octet-stream");
    request.responseType = "json";
    request.upload.onprogress = (event) => progress(event.loaded);
    request.onload = () => {
      if (request.status !== 200 || request.response?.bytes !== size) {
        fail(new Error(i18n._("error.upload")));
        return;
      }
      progress(size);
      cleanup();
      resolve();
    };
    request.onerror = () => fail(new Error(i18n._("error.upload")));
    request.onabort = () => fail(signal.reason || new Error(i18n._("error.upload")));
    signal.addEventListener("abort", abort, { once: true });
    try {
      request.send(new Uint8Array(size));
    } catch (error) {
      fail(error);
    }
  });
}

async function transferUpBatched() {
  const minChunk = 256 * 1024;
  const maxChunk = 16 * 1024 * 1024;
  const start = performance.now();
  const state = transferState();
  let acknowledged = 0;
  show(0, i18n._("status.testingUpload"), 0);

  async function stream(signal) {
    let chunkSize = minChunk;
    while (performance.now() - start < DURATION_MS) {
      const batchStarted = performance.now();
      await uploadBatch(chunkSize, signal, (bytes) => {
        state.bytes += bytes;
        updateTransfer(state, performance.now() - start, i18n._("status.testingUpload"));
      });
      acknowledged += 1;
      const seconds = Math.max(1, performance.now() - batchStarted) / 1000;
      const desired = chunkSize / seconds * 0.5;
      chunkSize = Math.round(
        Math.min(maxChunk, Math.max(minChunk, desired)) / minChunk,
      ) * minChunk;
    }
  }

  // Measure bytes sent during the interval, including unfinished final batches.
  // Waiting for every response can turn one stalled request into a failed test
  // or include idle response time in the measured upload rate.
  await runStreams(stream, DURATION_MS, true);
  if (acknowledged === 0) throw new Error(i18n._("error.upload"));
  return finishTransfer(state, start);
}

function updateTransfer(state, elapsed, label) {
  recordSpeedSample(state, elapsed);
  if (elapsed - state.displayedAt < DISPLAY_MS) return;
  show(
    state.bytes * 8 / (elapsed / 1000) / 1e6,
    label,
    elapsed / DURATION_MS * 100,
  );
  state.displayedAt = elapsed;
}

function transferState() {
  return {
    bytes: 0,
    displayedAt: 0,
    sampledBytes: 0,
    sampledAt: 0,
    samples: [],
  };
}

function recordSpeedSample(state, elapsed) {
  const interval = elapsed - state.sampledAt;
  const bytes = state.bytes - state.sampledBytes;
  if (interval < SAMPLE_MS || bytes <= 0) return;
  const mbps = bytes * 8 / (interval / 1000) / 1e6;
  state.samples.push([
    Number((elapsed / 1000).toFixed(3)),
    Number(mbps.toFixed(3)),
  ]);
  state.sampledBytes = state.bytes;
  state.sampledAt = elapsed;
}

function finishTransfer(state, start, speedBytes = state.bytes) {
  const elapsed = performance.now() - start;
  recordSpeedSample(state, elapsed);
  return {
    speed: speedBytes * 8 / (elapsed / 1000) / 1e6,
    samples: state.samples,
  };
}

function showRate(valueElement, unitElement, mbps) {
  const rate = i18n.rate(mbps);
  valueElement.textContent = rate.number;
  unitElement.textContent = rate.unit;
}

function resultText(result) {
  const mode = result.uploadMode === "streamed"
    ? i18n._("upload.streamed")
    : i18n._("upload.compatibility");
  return [
    `Speedtest · ${new Date(result.timestamp).toLocaleString(i18n.locale())}`,
    `${i18n._("metric.latency")}: ${result.latency.toFixed(1)} ms`,
    `${i18n._("metric.jitter")}: ${result.jitter.toFixed(1)} ms`,
    `${i18n._("metric.download")}: ${i18n.rate(result.download).text}`,
    `${i18n._("metric.upload")}: ${i18n.rate(result.upload).text} (${
      mode.toLocaleLowerCase(i18n.locale())
    })`,
    location.origin,
  ].join("\n");
}

async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // A denied Clipboard API may still permit the user-initiated fallback.
    }
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  try {
    if (!document.execCommand("copy")) throw new Error(i18n._("copy.failed"));
  } finally {
    textarea.remove();
  }
}

ui.copyResult.addEventListener("click", async () => {
  if (!currentResult) return;
  try {
    await copyText(resultText(currentResult));
    ui.copyResult.textContent = i18n._("copy.copied");
    setTimeout(() => ui.copyResult.textContent = i18n._("copy.result"), 1_500);
  } catch {
    ui.copyResult.textContent = i18n._("copy.failed");
  }
});

ui.start.addEventListener("click", async () => {
  ui.reading.hidden = false;
  ui.progressTrack.hidden = false;
  ui.metrics.hidden = false;
  ui.resultChart.hidden = true;
  ui.resultChart.replaceChildren();
  ui.resultActions.hidden = true;
  currentResult = null;
  ui.start.disabled = true;
  ui.start.querySelector("span").textContent = i18n._("status.testing");
  ui.latency.textContent =
    ui.jitter.textContent =
    ui.download.textContent =
    ui.upload.textContent =
      "—";
  ui.downloadUnit.textContent = ui.uploadUnit.textContent = "Mbps";
  resetStages();
  try {
    setStage("latency", "active");
    show(NaN, i18n._("status.warmingUp"), 0, "ms");
    await ping();
    const timing = await latency();
    ui.latency.textContent = timing.median.toFixed(1);
    ui.jitter.textContent = timing.jitter.toFixed(1);
    setStage("latency", "done");
    setStage("download", "active");
    const download = await transferDown();
    const down = download.speed;
    showRate(ui.download, ui.downloadUnit, down);
    setStage("download", "done");
    setStage("upload", "active");
    await pause(250);
    const upload = await transferUp();
    const up = upload.speed;
    showRate(ui.upload, ui.uploadUnit, up);
    setStage("upload", "done");
    currentResult = {
      timestamp: new Date().toISOString(),
      latency: timing.median,
      jitter: timing.jitter,
      download: down,
      upload: up,
      uploadMode: upload.mode,
      downloadSamples: download.samples,
      uploadSamples: upload.samples,
    };
    saveResult(currentResult);
    ui.resultChart.hidden = !renderSpeedChart(ui.resultChart, currentResult);
    ui.uploadMode.textContent = upload.mode === "streamed"
      ? i18n._("upload.streamed")
      : i18n._("upload.compatibility");
    ui.resultActions.hidden = false;
    show(NaN, i18n._("status.complete"), 100, "");
  } catch (error) {
    show(NaN, error.message || i18n._("status.failed"), 0, "");
  } finally {
    ui.start.disabled = false;
    ui.start.querySelector("span").textContent = i18n._("test.again");
  }
});
