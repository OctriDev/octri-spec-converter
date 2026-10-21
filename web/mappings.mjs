import { element, field } from "./dom.mjs";
import { parseJson, stringifyJson } from "../packages/browser/client.mjs";

function jsonValue(input, name) {
  if (!input.value.trim()) return undefined;
  try {
    return parseJson(input.value);
  } catch {
    throw new TypeError(`${name} must contain valid JSON.`);
  }
}

function streamFields(parent, label, binding, required) {
  const group = element("fieldset");
  group.append(
    element("legend", label + (required ? " (required)" : " (optional)")),
  );
  const framing = field(group, "Framing", binding?.framing || "", {
    options: ["", "ndjson", "json-sequence", "sse"],
  });
  const lifecycle = field(
    group,
    "Lifecycle and termination semantics",
    binding?.lifecycle || "",
    { multiline: true },
  );
  parent.append(group);
  return () => {
    if (!framing.value && !lifecycle.value.trim() && !required)
      return undefined;
    if (!framing.value || !lifecycle.value.trim())
      throw new TypeError(`${label} needs framing and lifecycle semantics.`);
    return { framing: framing.value, lifecycle: lifecycle.value.trim() };
  };
}

export function renderMappings(
  parent,
  requests,
  config,
  onSave,
  onError,
  navigate,
) {
  parent.replaceChildren();
  for (const request of requests) {
    const existing = config.httpMappings?.[request.key] || {};
    const details = element("details", undefined, "mapping");
    details.append(element("summary", request.key));
    const form = element("form");
    const source = element(
      "button",
      `Source: ${request.source.uri}${request.source.pointer}`,
      "source-link",
    );
    source.type = "button";
    source.addEventListener("click", () => navigate(request.source));
    form.append(source);
    const method = field(form, "HTTP method", existing.method || "");
    method.autocomplete = "off";
    method.setAttribute("autocapitalize", "none");
    const suggestions = element("datalist");
    suggestions.id = `${method.id}-suggestions`;
    for (const name of [
      "GET",
      "POST",
      "PUT",
      "PATCH",
      "DELETE",
      "HEAD",
      "OPTIONS",
      "TRACE",
      "QUERY",
    ]) {
      const option = element("option", name);
      option.value = name;
      suggestions.append(option);
    }
    method.setAttribute("list", suggestions.id);
    const methodHint = element(
      "p",
      "Method spelling is case-sensitive. Custom methods require OpenAPI 3.2.",
      "small muted",
    );
    methodHint.id = `${method.id}-hint`;
    method.setAttribute("aria-describedby", methodHint.id);
    form.append(suggestions, methodHint);
    const path = field(form, "HTTP path", existing.path || "");
    const server = field(
      form,
      "Server URL",
      existing.server || config.baseUrl || "",
      { type: "url" },
    );
    const status = field(
      form,
      "Response status",
      existing.responseStatus || "",
    );
    const security = field(
      form,
      "Security requirements (JSON; [] explicitly permits anonymous access)",
      existing.security === undefined ? "" : stringifyJson(existing.security),
      { multiline: true },
    );
    const location = request.payloadLocationRequired
      ? field(form, "Payload direction", existing.payloadLocation || "", {
          options: ["", "request", "response"],
        })
      : null;
    const bindings = [];
    if (request.inputFields.length) {
      const group = element("fieldset");
      group.append(element("legend", "Input field bindings"));
      group.append(
        element(
          "p",
          "Fields left in the body keep their declared names. Streaming requests bind whole messages.",
          "small muted",
        ),
      );
      for (const name of request.inputFields) {
        const current = existing.parameterBindings?.[name];
        const row = element("div", undefined, "field-row");
        const target = field(
          row,
          `${name}: location`,
          current?.location || "body",
          { options: ["body", "path", "query", "header", "cookie"] },
        );
        const wireName = field(
          row,
          "HTTP parameter name",
          current?.name || name,
        );
        group.append(row);
        bindings.push({ name, target, wireName });
      }
      form.append(group);
    }
    const requestStream = streamFields(
      form,
      "Request stream",
      existing.requestStream,
      request.requestStreamRequired,
    );
    const responseStream = streamFields(
      form,
      "Response stream",
      existing.responseStream,
      request.responseStreamRequired,
    );
    const schemas = element("details");
    schemas.append(element("summary", "Explicit schema replacements"));
    const requestSchema = field(
      schemas,
      "Request schema (JSON, optional)",
      existing.requestSchema === undefined
        ? ""
        : stringifyJson(existing.requestSchema, 2),
      { multiline: true },
    );
    const responseSchema = field(
      schemas,
      "Response schema (JSON, optional)",
      existing.responseSchema === undefined
        ? ""
        : stringifyJson(existing.responseSchema, 2),
      { multiline: true },
    );
    form.append(schemas);
    const submit = element(
      "button",
      "Apply mapping and reconvert",
      "secondary",
    );
    submit.type = "submit";
    form.append(submit);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      try {
        if (
          !method.value ||
          !path.value.trim() ||
          !server.value.trim() ||
          !status.value.trim()
        )
          throw new TypeError(
            "Complete method, path, server and response status.",
          );
        const mapping = {
          method: method.value,
          path: path.value.trim(),
          server: server.value.trim(),
          responseStatus: status.value.trim(),
        };
        for (const [key, value] of Object.entries({
          security: jsonValue(security, "Security"),
          requestSchema: jsonValue(requestSchema, "Request schema"),
          responseSchema: jsonValue(responseSchema, "Response schema"),
          requestStream: requestStream(),
          responseStream: responseStream(),
        }))
          if (value !== undefined) mapping[key] = value;
        if (location) {
          if (!location.value)
            throw new TypeError("Choose the payload direction.");
          mapping.payloadLocation = location.value;
        }
        const parameters = bindings.filter(
          ({ target }) => target.value !== "body",
        );
        if (parameters.length)
          mapping.parameterBindings = Object.fromEntries(
            parameters.map(({ name, target, wireName }) => [
              name,
              { location: target.value, name: wireName.value },
            ]),
          );
        await onSave(request.key, mapping);
      } catch (error) {
        onError(error);
      }
    });
    details.append(form);
    parent.append(details);
  }
}
