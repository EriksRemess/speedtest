import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

function page(script, overrides = {}) {
  const elements = new Map();
  const element = () => ({
    dataset: {}, style: {}, classList: { toggle() {}, add() {} },
    setAttribute() {}, addEventListener() {}, replaceChildren() {},
    append() {}, select() {}, remove() {}, querySelector: () => element(),
  });
  const context = vm.createContext({
    window: {}, navigator: { language: 'en' },
    location: { host: 'localhost', href: 'http://localhost/', search: '' },
    history: { replaceState() {} },
    localStorage: { getItem: () => null, setItem() {} },
    document: {
      cookie: '', documentElement: element(), body: element(),
      getElementById(id) {
        if (!elements.has(id)) elements.set(id, element());
        return elements.get(id);
      },
      querySelectorAll: () => [], createElement: element,
      execCommand: () => false,
    },
    matchMedia: () => ({ matches: false }),
    performance, setTimeout, clearTimeout, AbortController, Blob, URLSearchParams,
    ...overrides,
  });
  for (const file of ['locales/en.js', 'locales/lv.js', 'i18n.js', 'chart.js', ...(script ? [script] : [])]) {
    vm.runInContext(readFileSync(new URL(`../web/${file}`, import.meta.url), 'utf8'), context);
  }
  return context;
}

const valid = {
  timestamp: '2026-09-06T10:00:00Z', latency: 1, jitter: 0.1,
  download: 100, upload: 50,
};

test('malformed language cookies do not break initialization', () => {
  const context = page('');
  context.document.cookie = 'language=%E0%A4%A';
  assert.equal(vm.runInContext('detectLanguage()', context), 'en');
  context.navigator.language = 'lv-LV';
  assert.equal(vm.runInContext('detectLanguage()', context), 'lv');
});

test('history skips invalid dates and negative metrics', () => {
  const context = page('results.js', {
    localStorage: {
      getItem: (key) => key === 'results' ? JSON.stringify([
        valid, { ...valid, timestamp: 'bad date' }, { ...valid, upload: -1 },
      ]) : null,
      setItem() {},
    },
  });
  assert.equal(vm.runInContext('loadResults().length', context), 1);
});

test('failed transfers abort and settle all peers before returning', async () => {
  const context = page('app.js');
  let started = 0;
  let stopped = 0;
  context.stream = (signal) => {
    if (++started === 1) return Promise.reject(new Error('transfer failed'));
    return new Promise((resolve, reject) => signal.addEventListener('abort', () => {
      stopped++;
      reject(signal.reason);
    }, { once: true }));
  };
  await assert.rejects(vm.runInContext('runStreams(stream)', context), /transfer failed/);
  assert.equal(started, 4);
  assert.equal(stopped, 3);
});

test('stalled transfers time out and release every stream', async () => {
  const context = page('app.js');
  let stopped = 0;
  context.stream = (signal) => new Promise((resolve, reject) => {
    signal.addEventListener('abort', () => {
      stopped++;
      reject(signal.reason);
    });
  });
  await assert.rejects(vm.runInContext('runStreams(stream, 10)', context), /timed out/);
  assert.equal(stopped, 4);
});

test('timed downloads stop pending reads normally at the deadline', async () => {
  const context = page('app.js');
  context.stream = (signal) => new Promise((resolve, reject) => {
    signal.addEventListener('abort', () => reject(signal.reason));
  });
  assert.equal((await vm.runInContext('runStreams(stream, 10, true)', context)).length, 0);
});

test('ping rejects HTTP errors and invalid server responses', async () => {
  for (const response of [{ ok: false }, { ok: true, json: async () => ({ ok: false }) }]) {
    const context = page('app.js', { fetch: async () => response });
    await assert.rejects(vm.runInContext('ping()', context), /Latency test failed/);
  }
});

test('latency consumes the response and averages the middle two samples', async () => {
  let clock = 0;
  let sample = 0;
  const context = page('app.js', {
    performance: { now: () => clock },
    fetch: async () => ({ ok: true, json: async () => {
      clock += ++sample;
      return { ok: true };
    } }),
    setTimeout: (callback, ms) => setTimeout(callback, ms === 45 ? 0 : ms),
  });
  const result = await vm.runInContext('latency()', context);
  assert.equal(result.median, 5.5);
  assert.equal(result.jitter, 1);
});

test('copy reports a failed legacy operation and cleans up the textarea', async () => {
  const context = page('app.js');
  let removed = false;
  context.document.createElement = () => ({
    style: {}, select() {}, remove() { removed = true; },
  });
  await assert.rejects(vm.runInContext('copyText("result")', context), /Copy failed/);
  assert.equal(removed, true);
});

test('copy falls back after a Clipboard API denial', async () => {
  const context = page('app.js', {
    navigator: { language: 'en', clipboard: { writeText: async () => { throw Error('denied'); } } },
  });
  let copied = false;
  context.document.execCommand = () => { copied = true; return true; };
  await vm.runInContext('copyText("result")', context);
  assert.equal(copied, true);
});

test('translation catalogs have matching keys and placeholders', () => {
  const context = page('');
  const { en, lv } = context.window.translations;
  assert.deepEqual(Object.keys(en).sort(), Object.keys(lv).sort());
  for (const key of Object.keys(en)) {
    assert.deepEqual(en[key].match(/\{\w+\}/g), lv[key].match(/\{\w+\}/g), key);
  }
});

function uploadTransport(context, onSend) {
  const requests = [];
  context.XMLHttpRequest = class {
    upload = {};
    status = 200;
    open() {}
    setRequestHeader() {}
    send(body) {
      this.body = body;
      requests.push(this);
      onSend?.(this, requests.length);
    }
    abort() {
      this.aborted = true;
      this.onabort?.();
    }
  };
  return requests;
}

test('batch progress counts bytes once and cancellation releases the request', async () => {
  const context = page('app.js');
  const requests = uploadTransport(context);
  const controller = new AbortController();
  let bytes = 0;
  context.signal = controller.signal;
  context.onProgress = (delta) => { bytes += delta; };
  const batch = vm.runInContext('uploadBatch(100, signal, onProgress)', context);
  const request = requests[0];
  assert.ok(request.body instanceof Blob);
  assert.equal(request.body.size, 100);
  assert.equal(request.body.type, 'application/octet-stream');
  request.upload.onprogress({ loaded: 20 });
  request.upload.onprogress({ loaded: 20 });
  request.upload.onprogress({ loaded: 70 });
  assert.equal(bytes, 70);
  const rejection = assert.rejects(batch, /stop/);
  controller.abort(new Error('stop'));
  await rejection;
  assert.equal(request.aborted, true);
  assert.equal(request.upload.onprogress, null);
});

test('batch completion validates the status and acknowledged byte count', async () => {
  for (const [status, acknowledged, succeeds] of [[200, 100, true], [500, 100, false], [200, 90, false]]) {
    const context = page('app.js');
    const requests = uploadTransport(context);
    context.signal = new AbortController().signal;
    let bytes = 0;
    context.onProgress = (delta) => { bytes += delta; };
    const batch = vm.runInContext('uploadBatch(100, signal, onProgress)', context);
    const request = requests[0];
    request.status = status;
    request.response = { bytes: acknowledged };
    request.onload();
    if (succeeds) {
      await batch;
      assert.equal(bytes, 100);
    } else {
      await assert.rejects(batch, /Upload failed/);
    }
    assert.equal(request.onload, null);
  }
});

test('timed upload preserves measured progress when final responses stall', async () => {
  let clock = 0;
  let deadline;
  const context = page('app.js', {
    performance: { now: () => clock },
    setTimeout: (callback, ms) => { deadline = { callback, ms }; return 1; },
    clearTimeout() {},
  });
  const requests = uploadTransport(context, (request, index) => {
    if (index === 1) {
      clock = 250;
      request.response = { bytes: request.body.size };
      request.onload();
    }
  });
  const transfer = vm.runInContext('transferUpBatched()', context);
  // Settle the initial acknowledgment so this stream starts another batch.
  for (let i = 0; i < 5; i++) await Promise.resolve();
  assert.equal(deadline.ms, 5000);
  let sent = requests[0].body.size;
  clock = 4500;
  for (const request of requests.slice(1)) {
    const bytes = Math.floor(request.body.size / 2);
    sent += bytes;
    request.upload.onprogress({ loaded: bytes });
  }
  clock = 5000;
  deadline.callback();
  const result = await transfer;
  assert.equal(result.speed, sent * 8 / 5 / 1e6);
  assert.ok(requests.slice(1).every((request) => request.aborted));
});

test('timed upload cannot succeed without a server acknowledgment', async () => {
  let deadline;
  const context = page('app.js', {
    setTimeout: (callback) => { deadline = callback; return 1; },
    clearTimeout() {},
  });
  uploadTransport(context, (request) => request.upload.onprogress({ loaded: 1000 }));
  const transfer = vm.runInContext('transferUpBatched()', context);
  for (let i = 0; i < 5; i++) await Promise.resolve();
  deadline();
  await assert.rejects(transfer, /Upload failed/);
});
