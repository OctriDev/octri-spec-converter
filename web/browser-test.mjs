import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { chromium, firefox, webkit } from "playwright";
import AxeBuilder from "@axe-core/playwright";
import { serve } from "./serve.mjs";
import { postmanCredentialCases } from "../scripts/postman-auth-cases.mjs";

const { server, origin } = await serve();
const evidence = [];
const browserTypes = { chromium, firefox, webkit };
const selectedBrowser = process.env.OCTRI_BROWSER;
if (selectedBrowser && !Object.hasOwn(browserTypes, selectedBrowser))
  throw new Error(`Unsupported browser: ${selectedBrowser}`);
const modern =
  '{"openapi":"3.1.2","info":{"title":"Browser 猫","version":"1"},"paths":{},"components":{"schemas":{"Exact":{"type":"number","example":0.100000000000000005}}}}';
async function converted(page) {
  await page
    .getByRole("button", { name: "Convert specification", exact: true })
    .click();
  await page
    .locator("#state")
    .filter({ hasText: /complete|needs review|blocked/ })
    .waitFor();
  await page.locator("#convert:enabled").waitFor();
}

try {
  await mkdir(".impeccable/review", { recursive: true });
  await mkdir("artifacts/browser", { recursive: true });
  for (const [name, type] of Object.entries(browserTypes).filter(
    ([name]) => !selectedBrowser || name === selectedBrowser,
  )) {
    const browser = await type.launch();
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 1000 },
        acceptDownloads: true,
      });
      const page = await context.newPage();
      if (name === "chromium") {
        const icon = await context.newPage();
        const iconErrors = [];
        icon.on("console", (message) => {
          if (message.type() === "error") iconErrors.push(message.text());
        });
        const response = await icon.goto(origin + "/web/favicon.svg");
        assert.equal(response.status(), 200);
        assert.equal(response.headers()["content-type"], "image/svg+xml");
        assert.equal(
          await icon
            .locator("svg > path")
            .last()
            .evaluate((node) => getComputedStyle(node).fill),
          "rgb(255, 106, 0)",
          "Brand icon must retain its color under strict CSP",
        );
        assert.deepEqual(iconErrors, [], "Favicon CSP errors");
        await icon.close();
      }
      page.setDefaultTimeout(15000);
      const errors = [];
      const outbound = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      await page.addInitScript(() => {
        window.cspViolations = [];
        document.addEventListener("securitypolicyviolation", (event) => {
          // WebKit emits style-src-attr violations for its own native select
          // styling, even for a bare <select> without CSS or a style attribute.
          // Keep application violations visible without relaxing shipped CSP.
          window.cspViolations.push({
            nativeSelect:
              event.target?.tagName === "SELECT" &&
              !event.target.hasAttribute("style") &&
              event.effectiveDirective === "style-src-attr" &&
              event.blockedURI === "inline",
            directive: event.effectiveDirective,
            blockedURI: event.blockedURI,
            target: event.target?.tagName,
            sourceFile: event.sourceFile,
            line: event.lineNumber,
          });
        });
      });
      page.on("request", (request) => {
        if (new URL(request.url()).origin !== origin)
          outbound.push(request.url());
      });
      await page.goto(origin + "/web/index.html");
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#target option").count(), 12);
      await page.locator("#source").fill(
        JSON.stringify({
          openapi: "3.2.1",
          info: { title: "Method spelling", version: "1" },
          paths: {
            "/events": {
              additionalOperations: {
                COPY: {
                  operationId: "copyUpper",
                  responses: { 204: { description: "Copied" } },
                },
                copy: {
                  operationId: "copyLower",
                  responses: { 204: { description: "Copied" } },
                },
              },
            },
          },
        }),
      );
      await converted(page);
      const inspector = page.getByText("Inspect converted API", {
        exact: true,
      });
      await inspector.click();
      await page
        .getByRole("button", { name: "COPY /events · copyUpper", exact: true })
        .waitFor();
      await page
        .getByRole("button", { name: "copy /events · copyLower", exact: true })
        .waitFor();
      await inspector.click();
      await page.locator("#source").fill(modern);
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "complete");
      assert.match(
        await page.locator("#output").inputValue(),
        /0\.100000000000000005/,
      );
      await page.locator("#encoding").selectOption("yaml");
      assert.equal(await page.locator("#import").isDisabled(), true);
      await converted(page);
      assert.match(
        await page.locator("#output").inputValue(),
        /openapi: 3\.1\.2/,
      );
      assert.equal(await page.locator("#state").textContent(), "complete");
      const saved = page.waitForEvent("download");
      await page.locator("#download-report").click();
      const reportDownload = await saved;
      const savedReport = await readFile(await reportDownload.path());
      await page.locator("#convert:enabled").waitFor();
      await page.locator("#load-report").setInputFiles({
        name: "report.json",
        mimeType: "application/json",
        buffer: savedReport,
      });
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#state").textContent(), "complete");
      await page.locator("#compare-report").setInputFiles({
        name: "before.json",
        mimeType: "application/json",
        buffer: savedReport,
      });
      await page.locator("#comparison").waitFor({ state: "visible" });
      assert.match(
        await page.locator("#comparison-output").textContent(),
        /"changes": \[\]/,
      );

      // A comparison belongs to one report, including when a conversion is
      // repeated without editing its source or settings.
      await converted(page);
      assert.equal(await page.locator("#comparison").isHidden(), true);
      assert.equal(await page.locator("#comparison-output").textContent(), "");
      await page.locator("#compare-report").setInputFiles({
        name: "before.json",
        mimeType: "application/json",
        buffer: savedReport,
      });
      await page.locator("#comparison").waitFor({ state: "visible" });
      await page.locator("#source").fill(modern + "\n");
      assert.equal(await page.locator("#comparison").isHidden(), true);
      assert.equal(await page.locator("#comparison-output").textContent(), "");
      await converted(page);

      await page.locator("#sample").click();
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "needs review");
      let reviews = 0;
      while (await page.locator("[data-review-action]").count()) {
        assert.ok(reviews++ < 30, "review must converge");
        await page.locator("[data-review-action]").first().click();
        await page.locator("#convert:enabled").waitFor();
      }
      assert.equal(await page.locator("#state").textContent(), "complete");

      await page.locator("#source").fill(
        JSON.stringify({
          $kind: "collection",
          name: "Postman 3 browser",
          auth: { type: "noauth" },
          items: [
            {
              $kind: "http-request",
              name: "List pets",
              method: "GET",
              url: "https://api.example.com/postman-v3",
              examples: [
                {
                  $kind: "http-example",
                  name: "OK",
                  response: {
                    statusCode: 200,
                    statusText: "OK",
                    body: { type: "json", content: '{"pets":[]}' },
                  },
                },
              ],
            },
          ],
        }),
      );
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "needs review");
      assert.match(await page.locator("#output").inputValue(), /\/postman-v3/);
      reviews = 0;
      while (await page.locator("[data-review-action]").count()) {
        assert.ok(reviews++ < 30, "Postman 3 review must converge");
        await page.locator("[data-review-action]").first().click();
        await page.locator("#convert:enabled").waitFor();
      }
      assert.equal(await page.locator("#state").textContent(), "complete");
      // WebKit's screenshot synchronization injects an inline stylesheet.
      // Capture evidence in Chromium so that automation does not create a
      // false product CSP failure in the three-engine functional check.
      if (name === "chromium") {
        await page.screenshot({
          path: `artifacts/browser/${name}-postman-v3.png`,
          fullPage: true,
        });
      }

      await page
        .getByText("Metadata, variables and conversion settings", {
          exact: true,
        })
        .click();
      const credentialSource =
        postmanCredentialCases()[0].request.input.content;
      await page.locator("#source").fill(credentialSource);
      await page.locator("#input-format").selectOption("postman");
      await page.locator("#settings").fill('{"variables":{"region":"eu"}}');
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "blocked");
      assert.match(
        await page.locator("#credentials").textContent(),
        /List items/,
      );
      assert.ok(
        !(await page.locator("#credentials").textContent()).includes(
          "credential-secret-never-in-output",
        ),
      );
      await page
        .locator("#credentials")
        .getByLabel("Credential set", { exact: true })
        .selectOption("0");
      await page
        .getByRole("button", {
          name: "Use credentials and reconvert",
          exact: true,
        })
        .click();
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#state").textContent(), "needs review");
      assert.ok(
        !(await page.locator("#output").inputValue()).includes(
          "credential-secret-never-in-output",
        ),
      );
      reviews = 0;
      while (await page.locator("#decisions [data-review-action]").count()) {
        assert.ok(reviews++ < 30, "Credential review must converge");
        await page.locator("#decisions [data-review-action]").first().click();
        await page.locator("#convert:enabled").waitFor();
      }
      assert.equal(await page.locator("#state").textContent(), "complete");
      await page.locator("#source").fill(credentialSource + " ");
      assert.equal(
        await page
          .getByRole("button", {
            name: "Use credentials and reconvert",
            exact: true,
          })
          .isDisabled(),
        true,
      );
      assert.equal(await page.locator("#download-output").isDisabled(), true);
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "blocked");
      await page
        .getByRole("button", {
          name: "Remove saved credential choices and reconvert",
          exact: true,
        })
        .click();
      await page.locator("#convert:enabled").waitFor();
      const recovered = JSON.parse(
        await page.locator("#settings").inputValue(),
      );
      assert.equal(recovered.credentialSelections, undefined);
      assert.deepEqual(recovered.variables, { region: "eu" });
      await page
        .locator("#credentials")
        .getByLabel("Credential set", { exact: true })
        .selectOption("1");
      await page
        .getByRole("button", {
          name: "Use credentials and reconvert",
          exact: true,
        })
        .click();
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#state").textContent(), "needs review");
      const anonymous = JSON.parse(await page.locator("#output").inputValue());
      assert.deepEqual(anonymous.paths["/items"].get.security, []);
      const credentialAxe = await new AxeBuilder({ page })
        .include("#credentials")
        .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
        .analyze();
      assert.deepEqual(
        credentialAxe.violations,
        [],
        `${name}: credential accessibility`,
      );
      if (name === "chromium") {
        await page.evaluate(() => {
          document.activeElement?.blur();
          window.scrollTo(0, 0);
        });
        await page.screenshot({
          path: ".impeccable/review/credentials-desktop.png",
          fullPage: true,
          animations: "disabled",
        });
        await page.locator("#credentials").screenshot({
          path: ".impeccable/review/credentials-detail.png",
          animations: "disabled",
        });
        await page.setViewportSize({ width: 390, height: 844 });
        assert.equal(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
          true,
          "credential mobile overflow",
        );
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.screenshot({
          path: ".impeccable/review/credentials-mobile.png",
          fullPage: true,
          animations: "disabled",
        });
        await page.setViewportSize({ width: 1440, height: 1000 });
      }
      await page.locator("#source").fill("type Query { pet(id: ID!): String }");
      await page.locator("#input-format").selectOption("graphql");
      await page.locator("#settings").fill("{}");
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "blocked");
      await page.locator(".mapping summary").first().click();
      const mapping = page.locator(".mapping form").first();
      await mapping.getByLabel("HTTP method", { exact: true }).fill("GET");
      await mapping.getByLabel("HTTP path", { exact: true }).fill("/pets/{id}");
      await mapping
        .getByLabel("Server URL", { exact: true })
        .fill("https://api.example.com");
      await mapping.getByLabel("Response status", { exact: true }).fill("200");
      await mapping.getByLabel(/Security requirements/).fill("[]");
      await mapping
        .getByLabel("id: location", { exact: true })
        .selectOption("path");
      await mapping
        .getByRole("button", { name: "Apply mapping and reconvert" })
        .click();
      await page.locator("#convert:enabled").waitFor();
      assert.notEqual(await page.locator("#state").textContent(), "blocked");
      assert.match(
        await page.locator("#output").inputValue(),
        /\/pets\/\{id\}/,
      );

      await page.locator("#encoding").selectOption("json");
      await page.locator("#target").selectOption("3.2.1");
      await converted(page);
      for (const method of ["COPY", "copy", "get"]) {
        await page.locator(".mapping summary").first().click();
        await mapping.getByLabel("HTTP method", { exact: true }).fill(method);
        await mapping
          .getByRole("button", { name: "Apply mapping and reconvert" })
          .click();
        await page.locator("#convert:enabled").waitFor();
        assert.notEqual(await page.locator("#state").textContent(), "blocked");
        const output = JSON.parse(await page.locator("#output").inputValue());
        assert.ok(output.paths["/pets/{id}"].additionalOperations[method]);
        assert.equal(
          await mapping.getByLabel("HTTP method", { exact: true }).inputValue(),
          method,
        );
      }
      // Editing an existing operation keeps its authored HTTP contract. A new
      // operation offers explicit recovery without silently deleting settings.
      const savedConfig = JSON.parse(
        await page.locator("#settings").inputValue(),
      );
      await page
        .locator("#source")
        .fill("# Updated description\ntype Query { pet(id: ID!): String }");
      await converted(page);
      assert.deepEqual(
        JSON.parse(await page.locator("#settings").inputValue()),
        savedConfig,
      );
      assert.equal(await page.locator("#mapping-recovery").isVisible(), false);
      await page.locator("#source").fill("type Query { animals: [String] }");
      await page.locator("#convert").click();
      await page.locator("#mapping-recovery").waitFor();
      assert.match(
        await page.locator("#error").textContent(),
        /UNKNOWN_HTTP_MAPPING/,
      );
      assert.deepEqual(
        JSON.parse(await page.locator("#settings").inputValue()),
        savedConfig,
        "Failed conversion must retain authored configuration",
      );
      await page.locator("#reset-http-mappings").click();
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#state").textContent(), "blocked");
      assert.match(
        await page.locator("#mapping-list").textContent(),
        /animals/,
      );
      const { httpMappings, ...retainedConfig } = savedConfig;
      assert.ok(Object.keys(httpMappings).length);
      assert.deepEqual(
        JSON.parse(await page.locator("#settings").inputValue()),
        retainedConfig,
        "Recovery removes only HTTP mappings",
      );
      assert.equal(await page.locator("#mapping-recovery").isVisible(), false);
      assert.equal(await page.locator("#download-output").isEnabled(), true);
      assert.equal(
        await page.locator("#download-output").textContent(),
        "Download draft specification",
      );
      assert.equal(await page.locator("#import").isDisabled(), true);
      await page.locator("#target").selectOption("");

      const duplicates = {
        info: {
          name: "Duplicate requests",
          schema:
            "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
        },
        auth: { type: "noauth" },
        item: ["First", "Second", "Third"].map((name) => ({
          name,
          request: { method: "GET", url: "https://api.example.com/pets" },
        })),
      };
      await page.locator("#source").fill(JSON.stringify(duplicates));
      await page.locator("#input-format").selectOption("postman");
      await page.locator("#encoding").selectOption("json");
      await page.locator("#settings").fill("{}");
      await page.locator("#title").fill("Reviewed API");
      await page.locator("#api-version").fill("2026.1");
      await converted(page);
      assert.equal(await page.locator("#state").textContent(), "blocked");
      await page
        .getByLabel("Source request to retain", { exact: true })
        .selectOption("/item/2");
      await page
        .getByRole("button", {
          name: "Apply selection and reconvert",
          exact: true,
        })
        .click();
      await page.locator("#convert:enabled").waitFor();
      assert.equal(await page.locator("#state").textContent(), "needs review");
      const selected = JSON.parse(await page.locator("#output").inputValue());
      assert.equal(selected.paths["/pets"].get.summary, "Third");
      assert.equal(selected.info.title, "Reviewed API");
      assert.equal(selected.info.version, "2026.1");

      const axe = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
        .analyze();
      assert.deepEqual(
        axe.violations.map(({ id, nodes }) => ({ id, count: nodes.length })),
        [],
        `${name}: accessibility`,
      );
      if (name === "chromium") {
        await page.evaluate(() => {
          document.activeElement?.blur();
          window.scrollTo(0, 0);
        });
        await page.screenshot({
          path: ".impeccable/review/desktop.png",
          fullPage: true,
          animations: "disabled",
        });
        await page.setViewportSize({ width: 390, height: 844 });
        assert.equal(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
          true,
          "mobile overflow",
        );
        await page.screenshot({
          path: ".impeccable/review/mobile.png",
          fullPage: true,
          animations: "disabled",
        });
      }
      const violations = await page.evaluate(() => window.cspViolations);
      const nativeSelectWarnings =
        name === "webkit"
          ? violations.filter((item) => item.nativeSelect).length
          : 0;
      assert.deepEqual(
        violations.filter((item) => name !== "webkit" || !item.nativeSelect),
        [],
        `${name}: application CSP violations`,
      );
      let remainingNativeWarnings = nativeSelectWarnings;
      const productErrors = errors.filter((message) => {
        const nativeWarning =
          "Refused to apply a stylesheet because its hash, its nonce, or 'unsafe-inline' appears in neither the style-src directive nor the default-src directive of the Content Security Policy.";
        if (remainingNativeWarnings > 0 && message === nativeWarning) {
          remainingNativeWarnings--;
          return false;
        }
        return true;
      });
      assert.equal(
        remainingNativeWarnings,
        0,
        `${name}: unmatched native warnings`,
      );
      assert.deepEqual(productErrors, [], `${name}: browser errors`);
      assert.deepEqual(outbound, [], `${name}: conversion must stay offline`);
      evidence.push({
        browser: name,
        version: browser.version(),
        flows: [
          "exact decimals",
          "yaml output",
          "stale exports",
          "verified report download/open",
          "comparison",
          "individual review",
          "HTTP mapping",
          "source-bound credential selection and stale-choice recovery",
        ],
        axeViolations: 0,
        nativeSelectCspWarnings: nativeSelectWarnings,
      });
      console.log(
        `${name}: conversion, review, mapping, report round trip and accessibility passed`,
      );
    } finally {
      await browser.close();
    }
  }
  await writeFile(
    "artifacts/browser/ui.json",
    JSON.stringify(evidence, null, 2) + "\n",
  );
} finally {
  await new Promise((resolve) => server.close(resolve));
}
