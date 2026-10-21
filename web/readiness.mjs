import { element, button } from "./dom.mjs";

/** Readiness is static evidence about the output, separate from fidelity review. */
export function renderReadiness(parent, audit, handlers) {
  parent.replaceChildren();
  if (!audit) {
    parent.append(
      element(
        "p",
        "No readiness audit is attached. Swagger 2.0 receives strict parser validation; the Spec Auditor accepts OpenAPI 3.x.",
        "muted",
      ),
    );
    return;
  }
  parent.append(
    element(
      "p",
      `Spec Auditor: ${audit.status}. ${audit.findings.length} findings. These checks do not execute documentation, SDK or MCP generators.`,
    ),
  );
  const views = element("ul");
  for (const view of audit.views)
    views.append(
      element(
        "li",
        `${view.category}: ${view.status}; ${view.blockers} blockers; ${view.checked} checked; ${view.unsupported} unsupported.`,
      ),
    );
  parent.append(views);
  const coverage = element("details");
  coverage.append(element("summary", "Audit coverage and limits"));
  for (const line of audit.coverage)
    coverage.append(element("p", line, "small muted"));
  parent.append(coverage);
  const findings = element("div");
  parent.append(findings);
  let page = 0;
  function render() {
    findings.replaceChildren();
    for (const finding of audit.findings.slice(page * 40, (page + 1) * 40)) {
      const row = element("article", undefined, "decision");
      row.append(
        element("h4", `${finding.title} · ${finding.severity}`),
        element("p", finding.message),
        element("p", finding.remediation, "small"),
      );
      const output = finding.location.uri === audit.manifest.root;
      row.append(
        button(
          `${output ? "Output" : "Source"}: ${finding.location.pointer || "/"}`,
          () =>
            output
              ? handlers.output(finding.location.pointer)
              : handlers.navigate(finding.location),
          "source-link",
        ),
      );
      findings.append(row);
    }
    const navigation = element("div", undefined, "run-row");
    if (page)
      navigation.append(
        button("Previous audit findings", () => {
          page--;
          render();
        }),
      );
    if ((page + 1) * 40 < audit.findings.length)
      navigation.append(
        button("Next audit findings", () => {
          page++;
          render();
        }),
      );
    if (audit.findings.length)
      navigation.append(
        element(
          "span",
          `${page * 40 + 1}–${Math.min((page + 1) * 40, audit.findings.length)} of ${audit.findings.length}`,
          "small muted",
        ),
      );
    findings.append(navigation);
  }
  render();
}
