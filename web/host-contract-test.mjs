/** Node unit tests for bridge contracts; these do not qualify browser UI. */
import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { connectHost } from "./host.mjs";

const globals = ["window", "document", "location", "ResizeObserver"];
const originals = new Map(
  globals.map((name) => [
    name,
    Object.getOwnPropertyDescriptor(globalThis, name),
  ]),
);
afterEach(() => {
  for (const [name, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis, name, descriptor);
    else delete globalThis[name];
  }
});

function harness(importTargets) {
  const listeners = new Map();
  const sent = [];
  const loaded = [];
  const parent = {
    postMessage: (message, origin) => sent.push({ message, origin }),
  };
  globalThis.window = {
    parent,
    addEventListener: (type, listener) => listeners.set(type, listener),
  };
  globalThis.location = { origin: "https://octri.example" };
  const handoff = { hidden: true };
  globalThis.document = { getElementById: () => handoff, documentElement: {} };
  globalThis.ResizeObserver = class {
    observe() {}
    disconnect() {}
  };
  const host = connectHost({
    load: (source) => loaded.push(source),
    error: (failure) => {
      throw failure;
    },
  });
  const message = {
    type: "octri-converter:load",
    protocolVersion: 1,
    nonce: "host-contract-nonce-1234",
    expectedCurrentSpecId: null,
    importTargets,
    source: null,
  };
  const deliver = (data = message, origin = location.origin, source = parent) =>
    listeners.get("message")({ data, origin, source });
  host.ready();
  return { host, message, deliver, sent, loaded, handoff };
}

test("all advertised converter targets permit project handoff independently of SDK generation", () => {
  const versions = [
    "2.0",
    "3.0.0",
    "3.0.1",
    "3.0.2",
    "3.0.3",
    "3.0.4",
    "3.1.0",
    "3.1.1",
    "3.1.2",
    "3.2.0",
    "3.2.1",
  ];
  const { host, deliver, sent, loaded, handoff } = harness(versions);
  deliver();
  assert.equal(loaded.length, 1);
  assert.equal(handoff.hidden, false);
  for (const targetVersion of versions)
    assert.equal(host.importProblem({ manifest: { targetVersion } }), "");
  const exact =
    '{"manifest":{"targetVersion":"3.2.1"},"exact":0.100000000000000005}';
  host.handoff(exact);
  assert.equal(sent.at(-1).message.reportJson, exact);
  assert.equal(sent.at(-1).origin, "https://octri.example");
});

test("unadvertised versions stay downloadable and cannot replace an established host contract", () => {
  const { host, deliver, message } = harness(["3.1.2"]);
  deliver();
  const report = { manifest: { targetVersion: "3.2.1" } };
  assert.match(
    host.importProblem(report),
    /does not advertise this import version/,
  );
  deliver({ ...message, importTargets: ["3.2.1"] });
  assert.match(
    host.importProblem(report),
    /does not advertise this import version/,
  );
});

test("cross-origin and unrelated-window messages cannot connect a project", () => {
  const { host, deliver, message, loaded } = harness(["3.2.1"]);
  deliver(message, "https://unrelated.example");
  deliver(message, location.origin, {});
  assert.equal(loaded.length, 0);
  assert.throws(() => host.handoff("{}"), /No Octri project/);
});
