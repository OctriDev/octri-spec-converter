import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { chromium, firefox, webkit } from "playwright";
import { serve } from "./serve.mjs";

const { server, origin } = await serve();
const content =
  '{"openapi":"3.1.2","info":{"title":"Connected source","version":"1"},"paths":{},"x-exact":0.100000000000000005}';

async function verifyDownload(page, frame, targetVersion) {
  const hashLabel = await frame.locator("#output-hash").textContent();
  const [, expectedHash] =
    /^Artifact SHA-256 ([a-f0-9]{64})$/.exec(hashLabel) ?? [];
  assert.ok(expectedHash, "Completed conversion must expose its artifact hash");
  const pending = page.waitForEvent("download");
  await frame.locator("#download-output").click();
  const download = await pending;
  assert.equal(download.suggestedFilename(), `openapi-${targetVersion}.json`);
  const savedPath = await download.path();
  assert.ok(savedPath, "Embedded export must produce a saved file");
  const saved = await readFile(savedPath);
  assert.equal(createHash("sha256").update(saved).digest("hex"), expectedHash);
  assert.equal(JSON.parse(saved.toString("utf8")).openapi, targetVersion);
  assert.match(saved.toString("utf8"), /0\.100000000000000005/);
}

try {
  for (const [name, type] of Object.entries({ chromium, firefox, webkit })) {
    const browser = await type.launch();
    try {
      const page = await browser.newPage();
      const errors = [];
      page.on("pageerror", (failure) => errors.push(failure.message));
      await page.goto(origin + "/web/index.html");
      await page.locator("#convert:enabled").waitFor();
      await page.evaluate(() => {
        window.dispatchEvent(new Event("pagehide"));
        window.messages = [];
        window.addEventListener("message", (event) => {
          if (event.origin === location.origin)
            window.messages.push(event.data);
        });
        const frame = document.createElement("iframe");
        frame.src = "/web/index.html";
        frame.title = "Connected converter";
        frame.style.width = "1200px";
        frame.style.height = "1000px";
        document.body.replaceChildren(frame);
      });
      const frame = page.frameLocator("iframe");
      await page.waitForFunction(() =>
        window.messages.some(
          (message) => message.type === "octri-converter:ready",
        ),
      );
      await frame.locator("#convert:enabled").waitFor();
      const session = {
        type: "octri-converter:load",
        protocolVersion: 1,
        nonce: "local-qualification-session-1234",
        expectedCurrentSpecId: null,
        importTargets: ["3.1.2"],
        source: { uri: "memory://project/source.json", content },
      };
      const send = (message) =>
        page.evaluate(
          (message) =>
            document
              .querySelector("iframe")
              .contentWindow.postMessage(message, location.origin),
          message,
        );
      await send({ ...session, nonce: "short" });
      assert.equal(await frame.locator("#handoff").isHidden(), true);
      await send(session);
      await page.waitForFunction(
        (content) =>
          document
            .querySelector("iframe")
            .contentDocument.getElementById("source").value === content,
        content,
      );
      assert.equal(
        await frame.locator("#handoff").evaluate((node) => node.hidden),
        false,
      );
      assert.equal(await frame.locator("#source").inputValue(), content);
      await frame.locator("#convert").click();
      await frame.locator('#state[data-state="complete"]').waitFor();
      await frame.locator("#import:enabled").click();
      await page.waitForFunction(() =>
        window.messages.some(
          (message) => message.type === "octri-converter:handoff",
        ),
      );
      const handoff = await page.evaluate(() =>
        window.messages.find(
          (message) => message.type === "octri-converter:handoff",
        ),
      );
      assert.equal(handoff.nonce, session.nonce);
      assert.equal(handoff.expectedCurrentSpecId, null);
      assert.match(handoff.reportJson, /0\.100000000000000005/);
      assert.equal(
        JSON.parse(handoff.reportJson).original.input.content,
        content,
      );
      await verifyDownload(page, frame, "3.1.2");
      await frame.locator("#source").fill(content + "\n");
      await page.waitForFunction(() =>
        window.messages.some(
          (message) => message.type === "octri-converter:stale",
        ),
      );
      assert.equal(await frame.locator("#import").isDisabled(), true);
      await frame.locator("#target").selectOption("3.2.1");
      await frame.locator("#convert").click();
      await frame.locator('#state[data-state="complete"]').waitFor();
      await frame.locator("#convert:enabled").waitFor();
      assert.equal(await frame.locator("#download-output").isEnabled(), true);
      assert.equal(await frame.locator("#import").isDisabled(), true);
      assert.match(
        await frame.locator("#handoff-help").textContent(),
        /does not advertise this import version/,
      );
      await verifyDownload(page, frame, "3.2.1");
      await page.evaluate(() => {
        window.messages = [];
        document.querySelector("iframe").src = "/web/index.html";
      });
      await page.waitForFunction(() =>
        window.messages.some(
          (message) => message.type === "octri-converter:ready",
        ),
      );
      await send({ ...session, importTargets: ["3.1.2", "3.2.0", "3.2.1"] });
      await page.waitForFunction(
        (content) =>
          document
            .querySelector("iframe")
            .contentDocument.getElementById("source").value === content,
        content,
      );
      await frame.locator("#target").selectOption("3.2.1");
      await frame.locator("#convert").click();
      await frame.locator('#state[data-state="complete"]').waitFor();
      assert.equal(await frame.locator("#download-output").isEnabled(), true);
      assert.equal(await frame.locator("#import").isEnabled(), true);
      await frame.locator("#import").click();
      await page.waitForFunction(() =>
        window.messages.some(
          (message) =>
            message.type === "octri-converter:handoff" &&
            JSON.parse(message.reportJson).manifest.targetVersion === "3.2.1",
        ),
      );
      await verifyDownload(page, frame, "3.2.1");
      await page.evaluate(() => {
        window.messages = [];
        document.querySelector("iframe").src = "/web/index.html";
      });
      await page.waitForFunction(() =>
        window.messages.some(
          (message) => message.type === "octri-converter:ready",
        ),
      );
      await frame.locator("#convert:enabled").waitFor();
      await send({ ...session, maxReportBytes: 1 });
      await page.waitForFunction(
        (content) =>
          document
            .querySelector("iframe")
            .contentDocument.getElementById("source").value === content,
        content,
      );
      await frame.locator("#convert").click();
      await frame.locator('#state[data-state="complete"]').waitFor();
      assert.equal(await frame.locator("#download-output").isEnabled(), true);
      assert.equal(await frame.locator("#import").isDisabled(), true);
      assert.match(
        await frame.locator("#handoff-help").textContent(),
        /accepts reports up to/,
      );
      await verifyDownload(page, frame, "3.1.2");
      assert.deepEqual(errors, []);
      console.log(
        `${name}: nonce handshake, exact report handoff, stale approval, downstream compatibility and four hash-verified embedded downloads passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  await new Promise((resolve) => server.close(resolve));
}
