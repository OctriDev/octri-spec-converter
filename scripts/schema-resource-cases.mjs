/** Static resource migration and conservative scope guards for all consumers. */
export function schemaResourceCases() {
  const make = (schemas, targetVersion = "3.0.4", extra = {}, config = {}) => ({
    input: {
      uri: "memory://resources/api.json",
      content: JSON.stringify({
        openapi: "3.1.2",
        info: { title: "Resource migration", version: "1" },
        paths: {},
        components: { schemas },
        ...extra,
      }),
      generated: false,
    },
    files: {},
    config: { targetVersion, ...config },
  });
  const schemas = {
    Number: {
      $id: "https://schemas.example.test/number.json",
      $anchor: "Number",
      type: "integer",
      minimum: 1,
    },
    Alias: {
      $ref: "https://schemas.example.test/number.json#Number",
      maximum: 5,
    },
  };
  const cases = ["2.0", "3.0.4"].map((target) => ({
    kind: "static",
    request: make(schemas, target),
  }));
  const name = "猫 / %2F~ #?";
  const pointer = `/properties/${name.replaceAll("~", "~0").replaceAll("/", "~1")}`;
  cases.push({
    kind: "encoded",
    request: make({
      Object: {
        $id: "https://schemas.example.test/value.json",
        type: "object",
        properties: { [name]: { type: "integer", minimum: 1, maximum: 5 } },
      },
      Alias: {
        $ref: `https://schemas.example.test/value.json#${encodeURIComponent(pointer)}`,
      },
    }),
  });
  const legacy = {
    Number: {
      $schema: "http://json-schema.org/draft-07/schema#",
      $id: "https://schemas.example.test/number.json",
      type: "integer",
      minimum: 1,
      maximum: 5,
    },
    Alias: {
      $schema: "http://json-schema.org/draft-07/schema#",
      $ref: "https://schemas.example.test/number.json",
      type: "string",
      properties: { ignored: { $ref: "never-fetch.json" } },
    },
  };
  cases.push({ kind: "legacy", request: make(legacy) });
  cases.push({
    kind: "global",
    request: make(
      {
        Number: { type: "integer", minimum: 1, maximum: 5 },
        Alias: { $ref: "#/components/schemas/Number", type: "string" },
      },
      "3.0.4",
      { jsonSchemaDialect: "http://json-schema.org/draft-07/schema#" },
    ),
  });
  cases.push({
    kind: "dynamic",
    request: make({
      Node: {
        $id: "https://schemas.example.test/node.json",
        $dynamicAnchor: "node",
        type: "object",
        properties: { child: { $dynamicRef: "#node" } },
      },
    }),
  });
  cases.push({
    kind: "custom",
    request: make({
      Value: {
        $id: "https://schemas.example.test/custom.json",
        $schema: "https://dialects.example/custom",
        type: "object",
      },
    }),
  });
  cases.push({
    kind: "missing",
    request: make(
      schemas,
      "3.0.4",
      {},
      {
        overrides: {
          "/components/schemas/Alias": {
            $ref: "https://never-fetch.invalid/value.json",
          },
        },
      },
    ),
  });
  for (const schema of [
    {
      $id: "https://schemas.example.test/value.json",
      $schema: 3,
      type: "integer",
    },
    {
      $id: "https://schemas.example.test/value.json",
      type: "object",
      properties: { value: { $schema: 3, type: "integer" } },
    },
  ]) {
    cases.push({
      kind: "malformed",
      request: make(
        {
          Value: {
            $id: "https://schemas.example.test/value.json",
            type: "object",
          },
        },
        "3.0.4",
        {},
        { overrides: { "/components/schemas/Value": schema } },
      ),
    });
  }
  cases.push({
    kind: "invalid-unscoped",
    request: make(
      { Value: { type: "number", minimum: 2 } },
      "3.0.4",
      {},
      { overrides: { "/components/schemas/Value/exclusiveMinimum": false } },
    ),
  });
  return cases;
}
