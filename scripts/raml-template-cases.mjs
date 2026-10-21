/** Authored RAML template contracts shared by all runtime consumers. */
export function ramlTemplateCases() {
  const cases = [];
  for (const version of ["0.8", "1.0"]) {
    const declaration = version === "0.8" ? "  - payload:" : "  payload:";
    const schemaField = version === "0.8" ? "schema" : "type";
    const prefix = `#%RAML ${version}\ntitle: Trait schemas\nversion: v1\nbaseUri: https://api.example.test\n`;
    cases.push({
      kind: "included",
      request: {
        input: {
          uri: "memory://templates/api.raml",
          content: `${prefix}traits:\n${declaration} !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n`,
          generated: false,
        },
        files: {
          "traits/body.raml": `body:\n  application/json:\n    ${schemaField}: !include schemas/input.json\n`,
          "traits/schemas/input.json": JSON.stringify({
            $schema: "http://json-schema.org/draft-04/schema#",
            type: "array",
            items: { $ref: "value.json" },
          }),
          "traits/schemas/value.json": JSON.stringify({ type: "boolean" }),
        },
        config: { targetVersion: "3.2.1" },
      },
    });
  }
  const prefix =
    "#%RAML 0.8\ntitle: Templates\nversion: v1\nbaseUri: https://api.example.test\n";
  cases.push({
    kind: "parameterized",
    request: {
      input: {
        uri: "memory://templates/api.raml",
        content: `${prefix}traits:\n  - payload:\n      body:\n        application/json:\n          schema: {type: <<kind>>}\n/text:\n  post:\n    is: [{payload: {kind: string}}]\n    responses:\n      204: {description: Empty}\n/number:\n  post:\n    is: [{payload: {kind: integer}}]\n    responses:\n      204: {description: Empty}\n`,
        generated: false,
      },
      files: {},
      config: { targetVersion: "3.2.1" },
    },
  });
  cases.push({
    kind: "optional-inherited",
    request: {
      input: {
        uri: "memory://templates/api.raml",
        content: `${prefix}resourceTypes:\n  - base:\n      post?:\n        body:\n          application/json:\n            schema: !include value.json\n        responses:\n          204: {description: Empty}\n  - child:\n      type: base\n/items:\n  type: child\n  post:\n/absent:\n  type: child\n`,
        generated: false,
      },
      files: { "value.json": JSON.stringify({ type: "string" }) },
      config: { targetVersion: "3.2.1" },
    },
  });
  cases.push({
    kind: "template-error",
    expectedError: "SOURCE_INVALID",
    request: {
      input: {
        uri: "memory://templates/api.raml",
        content: `${prefix}traits:\n  - payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n`,
        generated: false,
      },
      files: {
        "traits/body.raml":
          'body:\n  application/json:\n    schema: \'{"$ref":"#/missing"}\'\n',
      },
      config: { targetVersion: "3.2.1" },
    },
  });
  const first = JSON.stringify(
    Array.from({ length: 128 }, (_, index) => index),
  );
  const second = JSON.stringify(
    Array.from({ length: 128 }, (_, index) => index + 64),
  );
  cases.push({
    kind: "array-merge",
    request: {
      input: {
        uri: "memory://templates/api.raml",
        content: `#%RAML 1.0\ntitle: Merged arrays\nversion: v1\nbaseUri: https://api.example.test\nresourceTypes:\n  values:\n    get:\n      queryParameters:\n        value: {type: integer, enum: ${first}}\n/items:\n  type: values\n  get:\n    queryParameters:\n      value: {enum: ${second}}\n    responses:\n      204: {description: Empty}\n`,
        generated: false,
      },
      files: {},
      config: { targetVersion: "3.2.1" },
    },
  });
  return cases;
}
