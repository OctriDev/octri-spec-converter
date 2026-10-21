import { element, field, button } from "./dom.mjs";

/** Browse the inventory already produced by strict output validation. */
export function renderInspection(parent, report, handlers) {
  parent.replaceChildren();
  const inventory = report.inspection;
  if (!inventory) {
    parent.append(
      element("p", "No complete output inventory is available.", "muted"),
    );
    return;
  }
  const stats = inventory.statistics;
  parent.append(
    element(
      "p",
      `${stats.operations} operations · ${stats.schemas} schemas · ${stats.references} references · ${stats.unresolved} unresolved`,
    ),
  );
  const search = field(parent, "Find operations or schemas", "", {
    type: "search",
  });
  const list = element("div");
  parent.append(list);
  const entries = [
    ...inventory.operations.map((operation) => ({
      label: `${operation.method} ${operation.path}${operation.operationId ? " · " + operation.operationId : ""}`,
      detail: operation.summary || operation.kind,
      location: operation.location,
    })),
    ...inventory.schemas.map((schema) => ({
      label: `Schema: ${schema.name}`,
      detail: `${schema.usedBy.length} uses`,
      location: schema.location,
    })),
  ];
  const validationRoot =
    !report.manifest.config.bundle &&
    ["openapi", "swagger"].includes(report.manifest.sourceFormat)
      ? report.manifest.root
      : report.output?.uri;
  let page = 0;
  function render() {
    const term = search.value.trim().toLowerCase();
    const selected = entries.filter(
      (entry) =>
        !term ||
        (entry.label + " " + entry.detail).toLowerCase().includes(term),
    );
    list.replaceChildren();
    for (const entry of selected.slice(page * 40, (page + 1) * 40)) {
      const row = element("div", undefined, "batch-row");
      row.append(
        button(
          entry.label,
          () =>
            entry.location.uri === validationRoot
              ? handlers.output(entry.location.pointer)
              : handlers.navigate(entry.location),
          "source-link",
        ),
        element("p", entry.detail, "small muted"),
      );
      list.append(row);
    }
    const navigation = element("div", undefined, "run-row");
    if (page)
      navigation.append(
        button("Previous inventory entries", () => {
          page--;
          render();
        }),
      );
    if ((page + 1) * 40 < selected.length)
      navigation.append(
        button("Next inventory entries", () => {
          page++;
          render();
        }),
      );
    navigation.append(
      element(
        "span",
        `${selected.length ? page * 40 + 1 : 0}–${Math.min((page + 1) * 40, selected.length)} of ${selected.length}`,
        "small muted",
      ),
    );
    list.append(navigation);
  }
  search.addEventListener("input", () => {
    page = 0;
    render();
  });
  render();
}
