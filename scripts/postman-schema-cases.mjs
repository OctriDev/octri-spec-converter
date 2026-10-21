/** Serialized body contracts shared by native, Node and browser consumers. */
export function postmanSchemaCases() {
  const make = (schema, files = {}, config = {}) => ({
    input: {
      uri: "memory://postman/collection.json",
      content: JSON.stringify(
        {
          $kind: "collection",
          name: "Scoped schemas",
          auth: { type: "noauth" },
          items: [
            {
              $kind: "http-request",
              name: "Create",
              method: "POST",
              url: "https://api.example.test/items",
              body: {
                type: "json",
                content: '{"value":2}',
                $type:
                  typeof schema === "string" ? schema : JSON.stringify(schema),
              },
              examples: [
                {
                  $kind: "http-example",
                  name: "Created",
                  response: { statusCode: 204, statusText: "Empty" },
                },
              ],
            },
          ],
        },
        null,
        2,
      ),
      generated: false,
    },
    files,
    config: { targetVersion: "3.2.1", ...config },
  });
  const cases = [];
  for (const [dialect, identifier, definitions] of [
    ["http://json-schema.org/draft-04/schema#", "id", "definitions"],
    ["http://json-schema.org/draft-07/schema#", "$id", "definitions"],
    ["https://json-schema.org/draft/2019-09/schema", "$id", "$defs"],
    ["https://json-schema.org/draft/2020-12/schema", "$id", "$defs"],
  ]) {
    cases.push({
      kind: "scoped",
      dialect,
      request: make(
        {
          $schema: dialect,
          [identifier]: "https://schemas.example.test/body.json",
          type: "object",
          required: ["value"],
          properties: { value: { $ref: `#/${definitions}/Value` } },
          [definitions]: { Value: { $ref: "value.json" } },
          examples: [{ $ref: "inert.json" }],
        },
        {
          "https://schemas.example.test/value.json":
            '{"type":"integer","minimum":1}',
        },
      ),
    });
  }
  cases.push({
    kind: "relative",
    request: make(
      { type: "object", properties: { value: { $ref: "schemas/value.json" } } },
      { "schemas/value.json": '{"type":"integer"}' },
    ),
  });
  cases.push({
    kind: "unknown-dialect",
    request: make({
      $schema: "https://dialects.example/custom",
      type: "object",
    }),
  });
  cases.push({
    kind: "missing-reference",
    expectedError: "SOURCE_INVALID",
    request: make({ $ref: "#/missing" }),
  });
  cases.push({
    kind: "syntax-error",
    expectedError: "SOURCE_INVALID",
    request: make("{"),
  });
  const limited = make(
    {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      enum: Array.from({ length: 400 }, (_, index) => index),
    },
    {},
    { limits: { maxDocuments: 1 } },
  );
  const source = JSON.parse(limited.input.content);
  source.items.push({
    ...source.items[0],
    url: "https://api.example.test/second",
  });
  limited.input.content = JSON.stringify(source);
  cases.push({
    kind: "bounded",
    expectedError: "SCHEMA_DERIVED_LIMIT",
    request: limited,
  });
  for (const schema of [
    {
      $id: "https://schemas.example.test/body.json",
      $schema: 3,
      type: "integer",
    },
    {
      $id: "https://schemas.example.test/body.json",
      type: "object",
      properties: { value: { $schema: 3, type: "integer" } },
    },
  ]) {
    cases.push({
      kind: "malformed-selector",
      request: make(schema, {}, { targetVersion: "3.0.4" }),
    });
  }
  return cases;
}
