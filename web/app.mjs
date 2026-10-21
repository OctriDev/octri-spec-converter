import { createWorkerClient } from "../packages/browser/worker-client.mjs";
import { parseJson, stringifyJson } from "../packages/browser/client.mjs";
import { $, element, button } from "./dom.mjs";
import {
  DEFAULT_URI,
  checkSources,
  uploadedSources,
  readText,
  download,
  fileName,
  selection,
  fetchSource,
} from "./files.mjs";
import { connectHost } from "./host.mjs";
import { renderReport, renderDecisions } from "./report.mjs";

const engine = createWorkerClient();
let files = new Map([[DEFAULT_URI, ""]]);
let root = DEFAULT_URI;
let active = DEFAULT_URI;
let revision = 0;
let reportRevision = -1;
let report;
let batchResults = [];
let busy = false;
let job = 0;
let controller;
let host;
const controls = [
  "convert",
  "batch",
  "fetch",
  "sample",
  "load-report",
  "compare-report",
  "load-config",
  "upload",
  "folder",
  "reset-http-mappings",
];
const exports = [
  "download-output",
  "copy-output",
  "download-report",
  "download-html",
  "download-manifest",
  "download-artifact",
  "import",
];
const endpointSetting = new URLSearchParams(location.search).get(
  "sourceEndpoint",
);
const sourceEndpoint =
  endpointSetting && /^\/api\/[a-zA-Z0-9/_-]+$/.test(endpointSetting)
    ? endpointSetting
    : undefined;

function current() {
  return !!report && revision === reportRevision;
}
function status(message) {
  $("status").textContent = message;
}
function showError(error) {
  $("error").hidden = false;
  $("error").textContent =
    `${error.code ? error.code + ": " : ""}${error.message || String(error)}`;
  $("mapping-recovery").hidden = error.code !== "UNKNOWN_HTTP_MAPPING";
}
function clearError() {
  $("error").hidden = true;
  $("error").textContent = "";
  $("mapping-recovery").hidden = true;
}
function clearComparison() {
  $("comparison").hidden = true;
  $("comparison-output").textContent = "";
}
function refreshActions() {
  for (const id of controls) $(id).disabled = busy;
  for (const id of exports)
    $(id).disabled =
      busy || !current() || (id === "import" && !report?.handoffAllowed);
  const importProblem = host?.importProblem(report) || "";
  $("import").disabled ||= !!importProblem;
  $("handoff-help").textContent =
    importProblem ||
    "Import sends this complete reviewed report and its original sources to your private Octri project. The server verifies it again before creating a specification version.";
  $("download-output").disabled ||= !report?.output;
  $("copy-output").disabled ||= !report?.output;
  $("cancel").hidden = !busy;
  $("progress").hidden = !busy;
  for (const node of document.querySelectorAll("[data-review-action]"))
    node.disabled = busy || !current();
}
function changed() {
  revision++;
  clearError();
  clearComparison();
  host?.stale();
  if (report) {
    $("state").textContent = "Stale — reconvert to review";
    $("state").dataset.state = "stale";
    status(
      "Sources or settings changed. Previous results remain visible; approval and exports are disabled.",
    );
  }
  refreshActions();
}

async function task(label, work) {
  if (busy) {
    showError(
      new Error("An operation is running. Cancel it or wait for completion."),
    );
    return;
  }
  busy = true;
  const id = ++job;
  const startedRevision = revision;
  controller = new AbortController();
  const operationController = controller;
  const timer = setTimeout(
    () =>
      operationController.abort(new Error("Operation exceeded 30 seconds.")),
    30000,
  );
  clearError();
  status(label);
  refreshActions();
  try {
    await work({
      revision: startedRevision,
      signal: operationController.signal,
      isLatest: () =>
        id === job &&
        startedRevision === revision &&
        !operationController.signal.aborted,
    });
  } catch (error) {
    if (id === job) {
      if (startedRevision === revision) {
        showError(error);
        status("Operation stopped. Correct the problem and retry.");
      } else {
        status(
          "Sources or settings changed. Reconvert to get current results.",
        );
      }
    }
  } finally {
    clearTimeout(timer);
    if (id === job) {
      busy = false;
      controller = undefined;
      refreshActions();
    }
  }
}

function configuration() {
  const extra = parseJson($("settings").value || "{}");
  if (!extra || typeof extra !== "object" || Array.isArray(extra))
    throw new TypeError("Additional configuration must be a JSON object.");
  return {
    ...extra,
    ...($("title").value.trim() ? { title: $("title").value.trim() } : {}),
    ...($("api-version").value.trim()
      ? { apiVersion: $("api-version").value.trim() }
      : {}),
    ...($("base-url").value.trim()
      ? { baseUrl: $("base-url").value.trim() }
      : {}),
    inputFormat: $("input-format").value,
    targetVersion: $("target").value || null,
    outputFormat: $("encoding").value,
    bundle: $("bundle").checked,
  };
}
function setConfiguration(config) {
  const {
    inputFormat = "auto",
    targetVersion,
    outputFormat = "json",
    bundle = true,
    title = "",
    apiVersion = "",
    baseUrl = "",
    ...extra
  } = config;
  $("input-format").value = inputFormat;
  $("target").value = targetVersion || "";
  $("encoding").value = outputFormat;
  $("bundle").checked = bundle;
  $("title").value = title || "";
  $("api-version").value = apiVersion || "";
  $("base-url").value = baseUrl || "";
  $("settings").value = stringifyJson(extra, 2);
}
function request(rootUri = root, config = configuration()) {
  checkSources(files);
  return {
    input: { uri: rootUri, content: files.get(rootUri), generated: false },
    files: Object.fromEntries([...files].filter(([uri]) => uri !== rootUri)),
    config,
  };
}
function refreshFiles() {
  for (const [id, selected] of [
    ["root", root],
    ["active", active],
  ]) {
    $(id).replaceChildren(
      ...[...files.keys()].map((uri) => {
        const option = element("option", uri.replace("memory://project/", ""));
        option.value = uri;
        option.selected = uri === selected;
        return option;
      }),
    );
  }
  $("source").value = files.get(active) || "";
  $("source-name").textContent = fileName(active);
  $("uri").value = active;
  $("remove").disabled = files.size === 1;
  $("batch-roots").replaceChildren(
    ...[...files.keys()].map((uri) => {
      const label = element("label", undefined, "check");
      const input = element("input");
      input.type = "checkbox";
      input.value = uri;
      input.checked = uri === root;
      label.append(
        input,
        document.createTextNode(uri.replace("memory://project/", "")),
      );
      return label;
    }),
  );
  position();
}
function position() {
  const before = $("source").value.slice(0, $("source").selectionStart);
  const lines = before.split("\n");
  $("position").textContent =
    `Line ${lines.length}, column ${Array.from(lines.at(-1)).length + 1}`;
}
function navigate(location) {
  if (!files.has(location.uri)) {
    showError(
      new Error(
        "That location is not in the supplied physical source inventory.",
      ),
    );
    return;
  }
  active = location.uri;
  refreshFiles();
  const at = selection($("source").value, location);
  $("source").focus();
  $("source").setSelectionRange(at, at);
  position();
}

function display(next, atRevision = revision) {
  clearComparison();
  report = next;
  reportRevision = atRevision;
  renderReport(report, report.manifest.config, {
    error: showError,
    navigate,
    isCurrent: () => current() && !busy,
    output: (pointer) => {
      let value = report.document;
      for (const part of pointer ? pointer.slice(1).split("/") : []) {
        const key = part.replaceAll("~1", "/").replaceAll("~0", "~");
        if (!value || typeof value !== "object" || !Object.hasOwn(value, key)) {
          showError(
            new Error(
              "This transformation has no current value at that output pointer.",
            ),
          );
          return;
        }
        value = value[key];
      }
      $("output-pointer").textContent = pointer || "/ (document)";
      $("output-value").textContent = stringifyJson(value, 2);
      $("output-location").hidden = false;
      $("output-location").open = true;
      $("output-value").focus();
    },
    accept: (id) => {
      if (!current() || busy) return;
      const config = configuration();
      config.acceptedDecisions = [
        ...new Set([...(config.acceptedDecisions || []), id]),
      ];
      setConfiguration(config);
      changed();
      convert("review");
    },
    duplicate: async (identity, pointer) => {
      const config = configuration();
      config.duplicateChoices = {
        ...config.duplicateChoices,
        [identity]: pointer,
      };
      setConfiguration(config);
      changed();
      await convert();
    },
    credential: async (pointer, choice) => {
      if (!current() || busy) return;
      const config = configuration();
      const entries = Object.entries(config.credentialSelections || {}).filter(
        ([key]) => key !== pointer,
      );
      if (choice !== null) entries.push([pointer, choice]);
      const choices = Object.fromEntries(entries);
      if (Object.keys(choices).length) config.credentialSelections = choices;
      else delete config.credentialSelections;
      setConfiguration(config);
      changed();
      await convert();
    },
    resetCredentials: async () => {
      if (!current() || busy) return;
      const config = configuration();
      delete config.credentialSelections;
      setConfiguration(config);
      changed();
      await convert();
    },
    mapping: async (key, mapping) => {
      const config = configuration();
      config.httpMappings = Object.fromEntries([
        ...Object.entries(config.httpMappings || {}).filter(
          ([name]) => name !== key,
        ),
        [key, mapping],
      ]);
      setConfiguration(config);
      changed();
      await convert();
    },
  });
  status(
    `${report.status.replaceAll("-", " ")}. ${report.decisions.filter((decision) => decision.reviewRequired && !decision.reviewed).length} decisions await review.`,
  );
  refreshActions();
}

async function convert(action = "convert") {
  await task("Converting and validating locally…", async (context) => {
    const next = await engine[action](request());
    if (!context.isLatest()) {
      status(
        "Sources changed during conversion. Reconvert to get current results.",
      );
      return;
    }
    display(next, context.revision);
  });
}

function restore(next) {
  const original = next.original;
  if (!original?.input?.content || !original.input.uri)
    throw new TypeError("Report has no usable original source.");
  const restored = new Map([
    [new URL(original.input.uri).href, original.input.content],
  ]);
  for (const [uri, content] of Object.entries(original.files || {})) {
    const absolute = new URL(uri, original.input.uri).href;
    if (restored.has(absolute) && restored.get(absolute) !== content)
      throw new TypeError("Report contains conflicting source identities.");
    restored.set(absolute, content);
  }
  checkSources(restored);
  files = restored;
  root = active = new URL(original.input.uri).href;
  setConfiguration(original.config || {});
  revision++;
  refreshFiles();
  display(next);
}

async function uploaded(event) {
  const selected = [...event.target.files];
  if (!selected.length) return;
  await task("Reading supplied files…", async (context) => {
    const empty =
      files.size === 1 && files.has(DEFAULT_URI) && !files.get(DEFAULT_URI);
    const next = await uploadedSources(selected, empty ? new Map() : files);
    if (!context.isLatest()) return;
    files = next;
    if (!files.has(root)) root = active = files.keys().next().value;
    changed();
    refreshFiles();
    status(
      `Loaded ${selected.length} source files. Choose the root and convert.`,
    );
  });
  event.target.value = "";
}

$("source").addEventListener("input", () => {
  files.set(active, $("source").value);
  changed();
  position();
});
for (const event of ["click", "keyup", "select"])
  $("source").addEventListener(event, position);
$("active").addEventListener("change", () => {
  active = $("active").value;
  refreshFiles();
});
$("root").addEventListener("change", () => {
  root = $("root").value;
  changed();
});
for (const id of [
  "input-format",
  "target",
  "encoding",
  "bundle",
  "settings",
  "title",
  "api-version",
  "base-url",
])
  $(id).addEventListener("input", changed);
$("convert").addEventListener("click", () => convert());
$("reset-http-mappings").addEventListener("click", () => {
  try {
    const config = configuration();
    delete config.httpMappings;
    setConfiguration(config);
    changed();
    convert();
  } catch (error) {
    showError(error);
  }
});
$("cancel").addEventListener("click", () => {
  job++;
  controller?.abort();
  engine.cancel();
  busy = false;
  refreshActions();
  status("Cancelled. No conversion result was applied.");
});
$("upload").addEventListener("change", uploaded);
$("folder").addEventListener("change", uploaded);
$("rename").addEventListener("click", () => {
  try {
    const uri = new URL($("uri").value).href;
    if (uri === active) return;
    if (files.has(uri))
      throw new TypeError("Another source already uses that URI.");
    const next = new Map(files);
    next.set(uri, files.get(active));
    next.delete(active);
    checkSources(next);
    if (root === active) root = uri;
    active = uri;
    files = next;
    changed();
    refreshFiles();
    clearError();
  } catch (error) {
    showError(error);
  }
});
$("remove").addEventListener("click", () => {
  if (files.size === 1) return;
  files.delete(active);
  if (root === active) root = files.keys().next().value;
  active = root;
  changed();
  refreshFiles();
});
$("search").addEventListener("input", () => renderDecisions(true));
$("filter").addEventListener("change", () => renderDecisions(true));
$("sample").addEventListener("click", () => {
  const uri = "memory://project/synthetic-postman.json";
  files = new Map([
    [
      uri,
      stringifyJson(
        {
          info: {
            name: "Synthetic pet collection",
            schema:
              "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
          },
          auth: { type: "noauth" },
          item: [
            {
              name: "List pets",
              request: { method: "GET", url: "https://api.example.com/pets" },
            },
          ],
        },
        2,
      ),
    ],
  ]);
  root = active = uri;
  setConfiguration({});
  changed();
  refreshFiles();
  status(
    "Synthetic Postman sample loaded. Convert to inspect inferred fields and review decisions.",
  );
});
$("fetch").addEventListener("click", () =>
  task("Fetching the requested URL…", async (context) => {
    const source = await fetchSource(
      $("url").value,
      context.signal,
      sourceEndpoint,
    );
    if (!context.isLatest()) return;
    const next = new Map(files);
    if (next.has(source.uri))
      throw new TypeError(
        "That source already exists. Remove it before fetching a replacement.",
      );
    if (next.size === 1 && !next.get(DEFAULT_URI)) next.clear();
    next.set(source.uri, source.content);
    checkSources(next);
    files = next;
    root = active = source.uri;
    changed();
    refreshFiles();
    status("Source fetched. Add any referenced files, then convert locally.");
  }),
);
$("save-config").addEventListener("click", () => {
  try {
    download("converter-config.json", stringifyJson(configuration(), 2));
  } catch (error) {
    showError(error);
  }
});
$("load-config").addEventListener("change", (event) =>
  task("Loading configuration…", async (context) => {
    const file = event.target.files[0];
    if (!file) return;
    const config = parseJson(await readText(file));
    if (!context.isLatest()) return;
    if (!config || typeof config !== "object" || Array.isArray(config))
      throw new TypeError("Configuration must be an object.");
    setConfiguration(config);
    changed();
    event.target.value = "";
    status("Configuration loaded. Reconvert to validate it.");
  }),
);
for (const [id, format] of [
  ["download-report", "json"],
  ["download-html", "html"],
  ["download-manifest", "manifest"],
  ["download-artifact", "artifact"],
]) {
  $(id).addEventListener("click", () => {
    if (!current()) return;
    task("Verifying portable artifact…", async (context) => {
      const output = await engine.export(report, format);
      if (!context.isLatest()) return;
      download(
        `conversion-${format}.${format === "html" ? "html" : "json"}`,
        typeof output === "string" ? output : stringifyJson(output, 2),
        format === "html" ? "text/html" : "application/json",
      );
      status(
        "Verified artifact downloaded. Original source text is included in review reports.",
      );
    });
  });
}
$("download-output").addEventListener("click", () => {
  if (!current() || !report.output) return;
  download(
    `openapi-${report.manifest.targetVersion}.${report.output.encoding}`,
    report.output.content,
    report.output.encoding === "yaml" ? "application/yaml" : "application/json",
  );
});
$("copy-output").addEventListener("click", async () => {
  if (!current() || !report.output) return;
  try {
    await navigator.clipboard.writeText(report.output.content);
    status("Output copied.");
  } catch {
    showError(
      new Error(
        "Clipboard access was denied. Select the output text or download it.",
      ),
    );
  }
});
$("load-report").addEventListener("change", (event) =>
  task("Recomputing saved report…", async (context) => {
    const file = event.target.files[0];
    if (!file) return;
    const saved = parseJson(await readText(file, 128 * 1024 * 1024));
    await engine.export(saved, "manifest");
    if (context.isLatest()) restore(saved);
    event.target.value = "";
  }),
);
$("compare-report").addEventListener("change", (event) => {
  if (!current()) {
    showError(
      new Error("Convert a current specification before comparing reports."),
    );
    return;
  }
  task("Verifying and comparing reports…", async (context) => {
    const file = event.target.files[0];
    if (!file) return;
    const before = parseJson(await readText(file, 128 * 1024 * 1024));
    const comparison = await engine.compare(before, report);
    if (!context.isLatest()) return;
    $("comparison-output").textContent = stringifyJson(comparison, 2);
    $("comparison").hidden = false;
    status("Both reports verified and compared.");
    event.target.value = "";
  });
});
$("batch").addEventListener("click", () =>
  task("Converting batch locally…", async (context) => {
    const roots = [...$("batch-roots").querySelectorAll("input:checked")].map(
      (input) => input.value,
    );
    if (!roots.length) throw new TypeError("Choose at least one batch root.");
    const config = configuration();
    const next = await engine.batch(roots.map((uri) => request(uri, config)));
    if (!context.isLatest()) return;
    batchResults = next;
    $("batch-list").replaceChildren(
      ...next.map((result, index) => {
        const row = element("div", undefined, "batch-row");
        row.append(
          element(
            "p",
            `${roots[index]}: ${result.Err ? result.Err.code + ": " + result.Err.message : result.Ok.status}`,
          ),
        );
        if (result.Ok)
          row.append(
            button("Open conversion for review", () => {
              restore(result.Ok);
            }),
          );
        return row;
      }),
    );
    $("batch-results").hidden = false;
    status(
      `Batch finished: ${next.filter((result) => result.Ok).length} conversions, ${next.filter((result) => result.Err).length} errors. Review each result before use.`,
    );
  }),
);
$("download-batch").addEventListener("click", () =>
  download("batch-reports.json", stringifyJson(batchResults, 2)),
);
window.addEventListener("pagehide", () => {
  controller?.abort();
  engine.dispose();
});

host = connectHost({
  error: showError,
  load(source) {
    const uri = source?.uri || DEFAULT_URI;
    const next = new Map([[uri, source?.content || ""]]);
    checkSources(next);
    files = next;
    root = active = uri;
    report = undefined;
    setConfiguration({ bundle: true });
    changed();
    $("report").hidden = true;
    $("empty").hidden = false;
    refreshFiles();
    refreshActions();
    status(
      source
        ? "Project source loaded. Convert and review before importing."
        : "New project connected. Add a source to convert and review.",
    );
  },
});
$("import").addEventListener("click", () =>
  task("Verifying reviewed Octri handoff…", async (context) => {
    if (!current() || !report.handoffAllowed || host.importProblem(report))
      return;
    const verified = await engine.export(report, "json");
    if (!context.isLatest()) return;
    host.handoff(stringifyJson(verified));
    status(
      "Reviewed report sent to Octri. Confirm import in the project workspace.",
    );
  }),
);
refreshFiles();
refreshActions();
const initialControls = [
  ...document.querySelectorAll("button,input,select,textarea"),
];
task("Loading local conversion engine…", async () => {
  const catalogue = await engine.formats();
  for (const format of catalogue.inputs) {
    const option = element("option", format);
    option.value = format;
    $("input-format").append(option);
  }
  for (const target of catalogue.targets) {
    const option = element("option", target);
    option.value = target;
    $("target").append(option);
  }
  $("target").value = "";
  status("Ready for your source. Conversion uses the local Rust engine.");
  host.ready();
}).finally(() => {
  for (const control of initialControls) control.disabled = false;
  refreshFiles();
  refreshActions();
});
