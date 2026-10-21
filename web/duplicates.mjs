import { element, field, button } from "./dom.mjs";

/** Structured choices come from Rust, never from diagnostic prose. */
export function renderDuplicates(parent, conflicts, handlers) {
  parent.replaceChildren();
  parent.hidden = !conflicts.length;
  if (!conflicts.length) return;
  parent.append(element("h3", "Resolve duplicate endpoints"));
  for (const conflict of conflicts) {
    const row = element("article", undefined, "decision");
    row.append(element("h4", conflict.identity));
    const choices = field(
      row,
      "Source request to retain",
      conflict.selected || "",
      {
        options: [
          "",
          ...conflict.candidates.map((candidate) => candidate.pointer),
        ],
      },
    );
    choices.options[0].textContent = "Choose a source request";
    for (const candidate of conflict.candidates)
      row.append(
        button(
          `View source ${candidate.pointer}`,
          () => handlers.navigate(candidate.source),
          "source-link",
        ),
      );
    row.append(
      button("Apply selection and reconvert", () => {
        if (!handlers.isCurrent() || !choices.value) return;
        handlers.duplicate(conflict.identity, choices.value);
      }),
    );
    parent.append(row);
  }
}
