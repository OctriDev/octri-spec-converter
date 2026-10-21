/** Deterministic controller tests. Browser layout and navigation need CUA/CI. */
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

class Control extends EventTarget {
  value = "";
  textContent = "";
  hidden = true;
  disabled = false;
  checked = false;
  selectionStart = 0;
  dataset = {};
  children = [];
  append(...children) {
    this.children.push(...children);
  }
  replaceChildren(...children) {
    this.children = children;
  }
}

test("mapping recovery preserves current settings and ignores obsolete failures", async () => {
  const names = ["document", "window", "location", "Worker"];
  const originals = new Map(
    names.map((name) => [
      name,
      Object.getOwnPropertyDescriptor(globalThis, name),
    ]),
  );
  const controls = new Map();
  const html = await readFile(new URL("./index.html", import.meta.url), "utf8");
  for (const [, id] of html.matchAll(/\bid="([^"]+)"/g)) {
    controls.set(id, new Control());
  }
  const get = (id) => {
    assert.ok(controls.has(id), `Unknown controller element: ${id}`);
    return controls.get(id);
  };
  const pending = [];
  let worker;
  class DeferredWorker {
    constructor() {
      worker = this;
    }
    postMessage(message) {
      const envelope = JSON.parse(message.envelope);
      if (envelope.action === "formats") {
        queueMicrotask(() =>
          this.reply(message.id, { Ok: { inputs: [], targets: [] } }),
        );
      } else {
        pending.push({ ...message, envelope });
      }
    }
    reply(id, response) {
      this.onmessage({ data: { id, response: JSON.stringify(response) } });
    }
    terminate() {}
  }
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const fire = async (id, type) => {
    get(id).dispatchEvent(new Event(type));
    await tick();
  };
  const fail = async (code) => {
    const request = pending.shift();
    assert.ok(request, "Controller must dispatch a worker request");
    worker.reply(request.id, {
      Err: { code, message: "Rejected test operation" },
    });
    await tick();
    return request.envelope.request;
  };
  try {
    globalThis.document = {
      getElementById: get,
      createElement: () => new Control(),
      createTextNode: (text) => ({ textContent: text }),
      querySelectorAll: (selector) =>
        selector === "button,input,select,textarea"
          ? [...controls.values()]
          : [],
    };
    globalThis.window = new EventTarget();
    window.parent = window;
    globalThis.location = { search: "", origin: "http://localhost" };
    globalThis.Worker = DeferredWorker;
    await import(`./app.mjs?controller-test=${Date.now()}`);
    await tick();
    assert.match(get("status").textContent, /Ready for your source/);
    const config = {
      variables: { region: "eu" },
      httpMappings: { old: { method: "POST", path: "/old" } },
    };
    get("settings").value = JSON.stringify(config);
    get("title").value = "Retain title";
    get("target").value = "3.2.1";
    get("source").value = "old source";
    await fire("source", "input");
    await fire("convert", "click");
    get("source").value = "new source";
    await fire("source", "input");
    await fail("UNKNOWN_HTTP_MAPPING");
    assert.equal(get("mapping-recovery").hidden, true);
    assert.equal(get("error").hidden, true);
    assert.match(get("status").textContent, /Reconvert/);
    assert.deepEqual(JSON.parse(get("settings").value), config);

    await fire("convert", "click");
    await fail("UNKNOWN_HTTP_MAPPING");
    assert.equal(get("mapping-recovery").hidden, false);
    assert.deepEqual(JSON.parse(get("settings").value), config);
    await fire("reset-http-mappings", "click");
    assert.equal(get("mapping-recovery").hidden, true);
    const request = await fail("INVALID_SOURCE");
    assert.equal(request.input.content, "new source");
    assert.equal(request.config.title, "Retain title");
    assert.equal(request.config.targetVersion, "3.2.1");
    assert.deepEqual(request.config.variables, config.variables);
    assert.equal(Object.hasOwn(request.config, "httpMappings"), false);
    assert.equal(get("mapping-recovery").hidden, true);
    assert.equal(get("error").hidden, false);
    await fire("source", "input");
    assert.equal(get("error").hidden, true);

    // Cancellation invalidates pending errors even when source stays unchanged.
    await fire("convert", "click");
    await fire("cancel", "click");
    await tick();
    assert.equal(get("error").hidden, true);
    assert.match(get("status").textContent, /Cancelled/);
  } finally {
    window.dispatchEvent(new Event("pagehide"));
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
});
