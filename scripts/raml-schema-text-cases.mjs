/** RAML 1.0 serialized schemas use the same scoped importer in every runtime. */
export function ramlSchemaTextCases() {
  const make = (schema, files = {}) => ({
    input: {
      uri: "memory://raml/api.raml",
      generated: false,
      content:
        "#%RAML 1.0\ntitle: Serialized schemas\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n",
    },
    files: {
      "traits/body.raml": `body:\n  application/json:\n    type: ${JSON.stringify(typeof schema === "string" ? schema : JSON.stringify(schema))}\n`,
      ...files,
    },
    config: { targetVersion: "3.2.1" },
  });
  const cases = [];
  for (const [dialect, definitions] of [
    ["http://json-schema.org/draft-04/schema#", "definitions"],
    ["http://json-schema.org/draft-07/schema#", "definitions"],
    ["https://json-schema.org/draft/2019-09/schema", "$defs"],
    ["https://json-schema.org/draft/2020-12/schema", "$defs"],
  ]) {
    cases.push({
      kind: "schema",
      dialect,
      request: make(
        {
          $schema: dialect,
          type: "object",
          properties: { value: { $ref: `#/${definitions}/Value` } },
          required: ["value"],
          [definitions]: { Value: { $ref: "value.json" } },
        },
        { "traits/value.json": '{"type":"integer","minimum":1}' },
      ),
    });
  }
  cases.push({
    kind: "syntax",
    expectedError: "SOURCE_INVALID",
    request: make("{"),
  });
  cases.push({
    kind: "reference",
    expectedError: "SOURCE_INVALID",
    request: make({ $ref: "#/missing" }),
  });
  const schema = {
    $schema: "http://json-schema.org/draft-07/schema#",
    type: "object",
    properties: { Code: { const: "Code" }, value: { $ref: "value.json" } },
    required: ["Code", "value"],
  };
  const library = make(schema);
  library.input.content =
    "#%RAML 1.0\ntitle: Libraries\nversion: v1\nbaseUri: https://api.example.test\nuses:\n  contracts: lib/contracts.raml\n/items:\n  post:\n    is: [contracts.payload]\n    responses:\n      204: {description: Empty}\n";
  library.files = {
    "lib/contracts.raml": `#%RAML 1.0 Library\ntypes:\n  Code: string\ntraits:\n  payload:\n    body:\n      application/json:\n        type: ${JSON.stringify(JSON.stringify(schema))}\n`,
    "lib/value.json": '{"type":"boolean"}',
  };
  cases.push({ kind: "library", dialect: schema.$schema, request: library });
  const quoted = JSON.stringify(JSON.stringify({ type: "string" }));
  for (const [kind, expectedError, declaration] of [
    [
      "query",
      "RAML_SCHEMA_PLACEMENT",
      `/items:\n  get:\n    queryParameters:\n      q:\n        type: ${quoted}\n`,
    ],
    [
      "xml-media",
      "RAML_SCHEMA_PLACEMENT",
      `/items:\n  post:\n    body:\n      application/xml:\n        type: ${quoted}\n`,
    ],
    [
      "specialization",
      "RAML_SCHEMA_SPECIALIZATION",
      `types:\n  Raw: ${quoted}\n  Wrapped:\n    type: ' Raw '\n    minLength: 2\n`,
    ],
    [
      "alias-query",
      "RAML_SCHEMA_PLACEMENT",
      `types:\n  Raw: ${quoted}\n  Alias: ' Raw '\n  Wrapped:\n    type: ' Alias '\n    description: Schema alias\n/items:\n  get:\n    queryParameters:\n      q: ' Wrapped '\n`,
    ],
    [
      "base-uri",
      "RAML_SCHEMA_PLACEMENT",
      `baseUriParameters:\n  tenant:\n    type: ${quoted}\n`,
    ],
  ]) {
    const request = make({});
    request.input.content = `#%RAML 1.0\ntitle: Placement\nversion: v1\nbaseUri: https://api.example.test\n${declaration}`;
    request.files = {};
    cases.push({ kind, expectedError, request });
  }
  for (const original of [...cases]) {
    const alias = structuredClone(original);
    alias.kind += "-alias";
    alias.expectedPointer = "/body/application~1json/schema";
    alias.request.input.content = alias.request.input.content.replaceAll(
      "type:",
      "schema:",
    );
    for (const [path, content] of Object.entries(alias.request.files)) {
      alias.request.files[path] = content
        .replaceAll("type:", "schema:")
        .replaceAll("types:", "schemas:");
    }
    cases.push(alias);
  }
  for (const declaration of [
    "types: {}\nschemas: {}\n",
    "types:\n  Value: {type: string, schema: string}\n",
    "/items:\n  post:\n    body:\n      application/json: {type: string, schema: string}\n",
  ]) {
    const request = make({});
    request.input.content = `#%RAML 1.0\ntitle: Collision\n${declaration}`;
    request.files = {};
    cases.push({
      kind: "alias-collision",
      expectedError: "RAML_TYPE_ALIAS",
      request,
    });
  }
  for (const original of cases.filter(({ expectedError }) => !expectedError)) {
    const bom = structuredClone(original);
    bom.kind = `bom-${bom.kind}`;
    for (const [path, content] of Object.entries(bom.request.files)) {
      if (path.endsWith(".raml"))
        bom.request.files[path] = content.replace(
          /^(\s*)(type|schema): (".*")$/gm,
          (_, indent, facet, encoded) =>
            `${indent}${facet}: ${JSON.stringify("\uFEFF" + JSON.parse(encoded))}`,
        );
    }
    cases.push(bom);
  }
  return cases;
}
