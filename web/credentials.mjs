import { button, element, field } from "./dom.mjs";

const PAGE_SIZE = 20;

/** Credential values stay in source. This editor handles names and indices only. */
export function renderCredentials(parent, requests, config, handlers) {
  parent.replaceChildren();
  const savedChoices = Object.keys(config.credentialSelections || {}).length;
  parent.hidden = !requests.length && !savedChoices;
  if (parent.hidden) return;
  parent.append(
    element("h3", "Choose request credentials"),
    element(
      "p",
      "Select the security contract for each request. Original matching rules remain in source; each choice needs review before handoff.",
      "small muted",
    ),
  );
  if (savedChoices) {
    const reset = button(
      "Remove saved credential choices and reconvert",
      async () => {
        if (!handlers.isCurrent()) return;
        try {
          await handlers.resetCredentials();
        } catch (error) {
          handlers.error(error);
        }
      },
    );
    reset.dataset.reviewAction = "true";
    reset.disabled = !handlers.isCurrent();
    parent.append(reset);
    if (!requests.length)
      parent.append(
        element(
          "p",
          "Saved choices no longer identify an inherited credential set in this source. Remove them before continuing.",
          "small",
        ),
      );
  }
  const list = element("div");
  const pages = element("div", undefined, "run-row");
  parent.append(list, pages);

  function showPage(page) {
    list.replaceChildren();
    for (const request of requests.slice(
      page * PAGE_SIZE,
      (page + 1) * PAGE_SIZE,
    )) {
      const form = element("form", undefined, "credential-choice");
      form.append(element("h4", request.name || request.request.pointer));
      form.append(
        button(
          "View request source",
          () => handlers.navigate(request.request),
          "source-link",
        ),
        button(
          "View inherited credentials",
          () => handlers.navigate(request.authSource),
          "source-link",
        ),
      );
      const select = field(form, "Credential set", "", { options: [] });
      const unset = element("option", "Choose a credential set");
      unset.value = "";
      select.append(unset);
      for (const choice of request.choices) {
        const name = choice.name || choice.id || "Credential set";
        const option = element(
          "option",
          `${choice.index + 1}. ${name} (${choice.authType})`,
        );
        option.value = String(choice.index);
        select.append(option);
      }
      select.value =
        request.selected === null || request.selected === undefined
          ? ""
          : String(request.selected);
      select.required = true;
      const save = element(
        "button",
        "Use credentials and reconvert",
        "secondary",
      );
      save.type = "submit";
      if (request.choices.length) save.dataset.reviewAction = "true";
      save.disabled = !request.choices.length || !handlers.isCurrent();
      form.append(save);
      if (!request.choices.length) {
        form.append(
          element(
            "p",
            "No credential sets are available. Correct the inherited auth source and reconvert.",
            "small",
          ),
        );
      }
      form.addEventListener("submit", async (event) => {
        event.preventDefault();
        if (!handlers.isCurrent()) return;
        const selected = request.choices.find(
          (choice) => String(choice.index) === select.value,
        );
        if (!selected) return;
        try {
          await handlers.credential(request.request.pointer, {
            sourceUri: request.authSource.uri,
            sourceSha256: request.sourceSha256,
            authPointer: request.authSource.pointer,
            credentialIndex: selected.index,
          });
        } catch (error) {
          handlers.error(error);
        }
      });
      if (
        Object.hasOwn(
          config.credentialSelections || {},
          request.request.pointer,
        )
      ) {
        const clear = button("Remove authored choice", async () => {
          if (!handlers.isCurrent()) return;
          try {
            await handlers.credential(request.request.pointer, null);
          } catch (error) {
            handlers.error(error);
          }
        });
        clear.dataset.reviewAction = "true";
        clear.disabled = !handlers.isCurrent();
        form.append(clear);
      }
      list.append(form);
    }
    pages.replaceChildren();
    if (requests.length <= PAGE_SIZE) return;
    const previous = button("Previous requests", () => showPage(page - 1));
    const next = button("Next requests", () => showPage(page + 1));
    previous.disabled = page === 0;
    next.disabled = (page + 1) * PAGE_SIZE >= requests.length;
    pages.append(
      previous,
      element(
        "span",
        `Requests ${page * PAGE_SIZE + 1}–${Math.min((page + 1) * PAGE_SIZE, requests.length)} of ${requests.length}`,
        "small muted",
      ),
      next,
    );
  }
  showPage(0);
}
