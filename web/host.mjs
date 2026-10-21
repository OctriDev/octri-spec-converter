/** Optional same-origin Octri bridge. Exact reports cross as JSON text. */
import { stringifyJson } from "../packages/browser/client.mjs";

export function connectHost({ load, error }) {
  const embedded = window.parent !== window;
  let ready = false;
  let pending;
  let session;
  const reportSizes = new WeakMap();
  const send = (type, fields = {}) => {
    if (embedded)
      window.parent.postMessage(
        { type: `octri-converter:${type}`, protocolVersion: 1, ...fields },
        location.origin,
      );
  };
  function initialize(message) {
    if (session || !ready) return;
    try {
      load(message.source);
      session = message;
      document.getElementById("handoff").hidden = false;
    } catch (failure) {
      error(failure);
      send("error", {
        nonce: message.nonce,
        message:
          "Project source exceeds converter limits or is malformed. Upload a supported source to continue.",
      });
    }
  }
  window.addEventListener("message", (event) => {
    if (
      !embedded ||
      event.origin !== location.origin ||
      event.source !== window.parent
    )
      return;
    const message = event.data;
    if (
      !message ||
      message.type !== "octri-converter:load" ||
      message.protocolVersion !== 1 ||
      typeof message.nonce !== "string" ||
      message.nonce.length < 16 ||
      message.nonce.length > 128 ||
      !(
        message.expectedCurrentSpecId === null ||
        (typeof message.expectedCurrentSpecId === "string" &&
          /^[a-f\d]{24}$/i.test(message.expectedCurrentSpecId))
      ) ||
      !Array.isArray(message.importTargets) ||
      !message.importTargets.every((target) => typeof target === "string") ||
      (message.maxReportBytes !== undefined &&
        (!Number.isSafeInteger(message.maxReportBytes) ||
          message.maxReportBytes < 1 ||
          message.maxReportBytes > 120 * 1024 * 1024)) ||
      !(
        message.source === null ||
        (typeof message.source?.uri === "string" &&
          typeof message.source?.content === "string")
      )
    )
      return;
    pending = message;
    initialize(message);
  });
  let resize;
  if (embedded) {
    resize = new ResizeObserver(() =>
      send("resize", { height: document.documentElement.scrollHeight }),
    );
    resize.observe(document.documentElement);
    window.addEventListener("pagehide", () => resize.disconnect(), {
      once: true,
    });
  }
  return {
    ready() {
      ready = true;
      if (pending) initialize(pending);
      send("ready");
    },
    stale() {
      if (session) send("stale", { nonce: session.nonce });
    },
    importProblem(report) {
      if (!session || !report) return "";
      if (session.maxReportBytes !== undefined) {
        if (!reportSizes.has(report))
          reportSizes.set(
            report,
            new TextEncoder().encode(stringifyJson(report)).length,
          );
        if (reportSizes.get(report) > session.maxReportBytes)
          return `This report is available for download. Connected project accepts reports up to ${Math.floor(session.maxReportBytes / 1024 / 1024)} MiB; reduce source size before importing.`;
      }
      return session.importTargets.includes(report?.manifest.targetVersion)
        ? ""
        : "This target is available for download. Connected project does not advertise this import version; choose one of its supported versions.";
    },
    handoff(reportJson) {
      if (!session) throw new Error("No Octri project is connected.");
      if (
        session.maxReportBytes !== undefined &&
        new TextEncoder().encode(reportJson).length > session.maxReportBytes
      )
        throw new Error("Report exceeds the connected project's import limit.");
      send("handoff", {
        nonce: session.nonce,
        expectedCurrentSpecId: session.expectedCurrentSpecId,
        reportJson,
      });
    },
  };
}
