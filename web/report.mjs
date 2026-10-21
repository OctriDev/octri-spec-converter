import { $, button, element } from "./dom.mjs";
import { renderMappings } from "./mappings.mjs";
import { renderReadiness } from "./readiness.mjs";
import { renderDuplicates } from "./duplicates.mjs";
import { renderInspection } from "./inspection.mjs";
import { renderCredentials } from "./credentials.mjs";
import { stringifyJson } from "../packages/browser/client.mjs";

const PAGE_SIZE = 40;
let page = 0;
let current;
let actions;

export function renderReport(report, config, handlers) {
  current = report;
  actions = handlers;
  page = 0;
  $("empty").hidden = true;
  $("report").hidden = false;
  $("state").textContent = report.status.replaceAll("-", " ");
  $("state").dataset.state = report.status;
  $("summary").textContent = report.valid
    ? `Valid ${report.manifest.targetVersion} output. ${report.handoffAllowed ? "Conversion review complete." : "Resolve blockers and review consequential decisions before handoff."}`
    : "Output failed strict validation. Correct the reported problems before use.";
  $("checks").replaceChildren(
    ...report.checks.map((check) => {
      const node = element("span", `${check.id}: ${check.status}`);
      node.dataset.status = check.status;
      node.title = check.detail;
      return node;
    }),
  );
  $("mappings").hidden = !report.mappingRequests.length;
  renderDuplicates($("duplicates"), report.duplicateRequests || [], handlers);
  renderReadiness($("readiness"), report.audit, handlers);
  renderInspection($("inspection"), report, handlers);
  renderCredentials(
    $("credentials"),
    report.credentialSelectionRequests || [],
    config,
    handlers,
  );
  renderMappings(
    $("mapping-list"),
    report.mappingRequests,
    config,
    handlers.mapping,
    handlers.error,
    handlers.navigate,
  );
  $("output").value = report.output?.content || "";
  if (report.output?.encoding === "json") {
    try {
      $("output").value = stringifyJson(report.document, 2);
    } catch {
      /* Large documents retain their bounded original representation. */
    }
  }
  $("output-location").hidden = true;
  $("output-hash").textContent = report.output
    ? `Artifact SHA-256 ${report.output.sha256}`
    : "No output";
  $("download-output").textContent = report.handoffAllowed
    ? "Download specification"
    : "Download draft specification";
  const diagnosticNodes = report.diagnostics.map((diagnostic) => {
    const row = element("p");
    row.append(
      element("strong", diagnostic.code + ": "),
      document.createTextNode(diagnostic.message + " "),
    );
    row.append(
      button(
        `${diagnostic.location.uri}${diagnostic.location.pointer}`,
        () => handlers.navigate(diagnostic.location),
        "source-link",
      ),
    );
    return row;
  });
  $("diagnostics").replaceChildren(
    ...(diagnosticNodes.length
      ? diagnosticNodes
      : [element("p", "No validation diagnostics.", "muted")]),
  );
  renderDecisions();
}

export function renderDecisions(reset = false) {
  if (!current) return;
  if (reset) page = 0;
  const term = $("search").value.trim().toLowerCase();
  const filter = $("filter").value;
  const decisions = current.decisions.filter((decision) => {
    const selected =
      filter === "all" ||
      (filter === "blocked" && decision.classification === "blocked") ||
      (filter === "pending" &&
        ((decision.reviewRequired && !decision.reviewed) ||
          decision.classification === "blocked")) ||
      (filter === "reviewed" && decision.reviewed);
    return (
      selected &&
      (!term ||
        `${decision.code} ${decision.message} ${decision.source.uri} ${decision.source.pointer}`
          .toLowerCase()
          .includes(term))
    );
  });
  const pending = current.decisions.filter(
    (decision) => decision.reviewRequired && !decision.reviewed,
  ).length;
  const blocked = current.decisions.filter(
    (decision) => decision.classification === "blocked",
  ).length;
  $("review-count").textContent =
    `${pending} pending · ${blocked} blocked · ${current.decisions.length} total`;
  page = Math.min(
    page,
    Math.max(0, Math.ceil(decisions.length / PAGE_SIZE) - 1),
  );
  const nodes = decisions
    .slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE)
    .map((decision) => {
      const article = element("article", undefined, "decision");
      article.dataset.classification = decision.classification;
      article.append(
        element("h4", decision.code),
        element("p", decision.message),
      );
      article.append(
        element(
          "p",
          `${decision.classification} · ${decision.reviewed ? "reviewed" : decision.reviewRequired ? "review required" : "recorded"}`,
          "meta",
        ),
      );
      article.append(
        button(
          `${decision.source.uri}${decision.source.pointer}`,
          () => actions.navigate(decision.source),
          "source-link",
        ),
      );
      for (const pointer of decision.outputPointers)
        article.append(
          button(
            `Output: ${pointer || "/ (document)"}`,
            () => actions.output(pointer),
            "source-link",
          ),
        );
      if (decision.remediation)
        article.append(element("p", decision.remediation, "small"));
      if (
        decision.reviewRequired &&
        !decision.reviewed &&
        decision.classification !== "blocked"
      ) {
        const accept = button("Accept this decision and revalidate", () =>
          actions.accept(decision.id),
        );
        accept.dataset.reviewAction = "true";
        accept.disabled = !actions.isCurrent();
        article.append(accept);
      }
      return article;
    });
  $("decisions").replaceChildren(
    ...(nodes.length
      ? nodes
      : [element("p", "No decisions match this view.", "muted")]),
  );
  const navigation = [];
  if (page)
    navigation.push(
      button("Previous decisions", () => {
        page--;
        renderDecisions();
      }),
    );
  if ((page + 1) * PAGE_SIZE < decisions.length)
    navigation.push(
      button("Next decisions", () => {
        page++;
        renderDecisions();
      }),
    );
  navigation.push(
    element(
      "span",
      `${decisions.length ? page * PAGE_SIZE + 1 : 0}–${Math.min((page + 1) * PAGE_SIZE, decisions.length)} of ${decisions.length}`,
      "small muted",
    ),
  );
  $("decision-pages").replaceChildren(...navigation);
}
