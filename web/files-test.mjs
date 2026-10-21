/** Download ownership must not require access to a foreign embedding page. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  download,
  fetchSource,
  readText,
  selection,
  uploadedSources,
} from "./files.mjs";

test("uploaded sources preserve BOM bytes and diagnostic byte offsets", async () => {
  const source = "\uFEFF猫<schema/>";
  const file = new File([source], "schema.xsd");
  assert.equal(await readText(file), source);
  const sources = await uploadedSources([file], new Map());
  assert.equal(sources.get("memory://project/schema.xsd"), source);
  assert.equal(selection(source, { offset: 3 }), 1);
  assert.equal(selection(source, { offset: 6 }), 2);
  await assert.rejects(
    readText(new File([new Uint8Array([0xff])], "bad.xsd")),
    TypeError,
  );
});

test("fetched sources preserve a BOM split across streamed chunks", async (context) => {
  const source = "\uFEFF<schema>猫</schema>";
  const encoded = new TextEncoder().encode(source);
  context.mock.method(
    globalThis,
    "fetch",
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(encoded.slice(0, 1));
            controller.enqueue(encoded.slice(1, 2));
            controller.enqueue(encoded.slice(2));
            controller.close();
          },
        }),
        {
          headers: {
            "x-octri-source-url": "https://source.example/schema.xsd",
          },
        },
      ),
  );
  assert.deepEqual(await fetchSource("https://source.example/schema.xsd"), {
    uri: "https://source.example/schema.xsd",
    content: source,
  });
});

test("downloads preserve content and remain within same-origin document authority", async (context) => {
  const globals = ["window", "document"].map((key) => [
    key,
    Object.getOwnPropertyDescriptor(globalThis, key),
  ]);
  const blobs = new Map();
  const revoked = [];
  const timers = [];
  context.mock.method(URL, "createObjectURL", (blob) => {
    const uri = `blob:download-${blobs.size}`;
    blobs.set(uri, blob);
    return uri;
  });
  context.mock.method(URL, "revokeObjectURL", (uri) => revoked.push(uri));
  context.mock.method(globalThis, "setTimeout", (callback) =>
    timers.push(callback),
  );
  function document() {
    return {
      anchors: [],
      createElement(tag) {
        assert.equal(tag, "a");
        return {
          click() {
            this.clicked = true;
          },
          remove() {
            this.removed = true;
          },
        };
      },
      body: {
        append(anchor) {
          this.owner.anchors.push(anchor);
        },
      },
    };
  }
  try {
    for (const kind of [
      "standalone",
      "same-origin",
      "different-origin",
      "inaccessible-parent",
    ]) {
      const local = document();
      const parent = document();
      local.body.owner = local;
      parent.body.owner = parent;
      const view = { location: { origin: "https://converter.example" } };
      view.parent =
        kind === "standalone"
          ? view
          : {
              location: {
                origin:
                  kind === "different-origin"
                    ? "https://other.example"
                    : view.location.origin,
              },
              document: parent,
            };
      if (kind === "inaccessible-parent")
        Object.defineProperty(view.parent, "location", {
          get() {
            throw new DOMException("Cross-origin", "SecurityError");
          },
        });
      globalThis.window = view;
      globalThis.document = local;
      download(
        "api / 猫.json",
        '{"value":18446744073709551615}',
        "application/json",
      );
      const owner = kind === "same-origin" ? parent : local;
      const other = owner === local ? parent : local;
      assert.equal(owner.anchors.length, 1, kind);
      assert.equal(other.anchors.length, 0, kind);
      const anchor = owner.anchors[0];
      assert.equal(anchor.download, "api____.json");
      assert.equal(anchor.clicked, true);
      assert.equal(anchor.removed, true);
      const blob = blobs.get(anchor.href);
      assert.equal(blob.type, "application/json");
      assert.equal(await blob.text(), '{"value":18446744073709551615}');
    }
    assert.equal(
      revoked.length,
      0,
      "Object URLs must survive the synchronous click",
    );
    for (const callback of timers) callback();
    assert.deepEqual(revoked, [...blobs.keys()]);
  } finally {
    for (const [key, descriptor] of globals) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
