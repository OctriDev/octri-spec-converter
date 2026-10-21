/** RAML 0.8 contracts shared by native, installed-package and WASM consumers. */
export function raml08Cases() {
  const legacySchema = {
    $schema: "http://json-schema.org/draft-04/schema#",
    id: "https://schemas.example.test/payload.json",
    type: "object",
    definitions: {
      Value: { type: "number", minimum: 0, exclusiveMinimum: true },
    },
    properties: {
      value: { $ref: "#/definitions/Value" },
      pair: {
        type: "array",
        items: [{ type: "string" }, { type: "integer" }],
        additionalItems: false,
      },
    },
    required: ["value"],
  };
  const prefix = `#%RAML 0.8
title: Legacy API
version: 0.8
baseUri: https://api.example.test/{version}
`;
  const transport = `${prefix}traits:
  - paged:
      queryParameters:
        limit: {type: integer, maximum: 100}
/items:
  get:
    is: [paged]
    queryParameters:
      tags: {repeat: true}
    responses:
      204:
        description: Empty
`;
  const cases = [];
  for (const schema of ["{", JSON.stringify({ $ref: "#/missing" })]) {
    cases.push({
      kind: "invalid-embedded-schema",
      expectedError: "SOURCE_INVALID",
      expectedPointer: "/~1items/post/body/application~1json/schema",
      request: {
        input: {
          uri: "memory://fixtures/legacy.raml",
          content: `${prefix}/items:\n  post:\n    body:\n      application/json:\n        schema: ${JSON.stringify(schema)}\n    responses:\n      204: {description: Empty}\n`,
          generated: false,
        },
        files: {},
        config: {},
      },
    });
  }
  cases.push({
    kind: "declaration-origin-collision",
    request: {
      input: {
        uri: "memory://fixtures/legacy.raml",
        content: `${prefix}traits:\n  - "0":\n      queryParameters:\n        unused: {type: string}\n    queryParameters:\n      queryParameters:\n        used: {type: integer}\n/items:\n  get:\n    is: [queryParameters]\n    responses:\n      204: {description: Empty}\n`,
        generated: false,
      },
      files: {},
      config: {},
    },
  });
  for (const targetVersion of ["3.1.2", "3.2.1"]) {
    for (const kind of ["transport", "schema-included", "schema-inline"]) {
      const inline = kind === "schema-inline";
      const content =
        kind === "transport"
          ? transport
          : `${prefix}schemas:
  - Payload: ${inline ? JSON.stringify(JSON.stringify(legacySchema)) : "!include payload.json"}
/items:
  post:
    body:
      application/json:
        schema: Payload
    responses:
      204:
        description: Empty
`;
      cases.push({
        kind,
        request: {
          input: {
            uri: "memory://fixtures/legacy.raml",
            content,
            generated: false,
          },
          files:
            kind === "schema-included"
              ? { "payload.json": JSON.stringify(legacySchema) }
              : {},
          config: { targetVersion },
        },
      });
    }
  }
  const schema = cases.find(({ kind }) => kind === "schema-inline").request;
  cases.push({
    kind: "schema-downgrade",
    request: { ...schema, config: { targetVersion: "3.0.4" } },
  });
  cases.push({
    kind: "schema-unknown-dialect",
    request: {
      ...schema,
      input: {
        ...schema.input,
        content: schema.input.content.replace(
          "http://json-schema.org/draft-04/schema#",
          "urn:unsupported-schema",
        ),
      },
    },
  });
  for (const [kind, body, expectedError] of [
    [
      "invalid-form-schema",
      '    body:\n      multipart/form-data:\n        schema: \'{"type":"object"}\'',
      "RAML_FORM",
    ],
    [
      "invalid-parameter-type",
      "    queryParameters:\n      kind:\n        type: [string, integer]",
      "RAML_PARAMETER",
    ],
  ]) {
    cases.push({
      kind,
      expectedError,
      request: {
        input: {
          uri: "memory://fixtures/legacy.raml",
          content: `${prefix}/items:\n  post:\n${body}\n    responses:\n      204: {description: Empty}\n`,
          generated: false,
        },
        files: {},
        config: {},
      },
    });
  }
  cases.push({
    kind: "unknown-body-examples",
    request: {
      input: {
        uri: "memory://fixtures/legacy.raml",
        content: `${prefix}/items:\n  post:\n    body:\n      application/json:\n        example: {value: 7, extra: true}\n        examples:\n          modern: {value: 8}\n    responses:\n      204: {description: Empty}\n`,
        generated: false,
      },
      files: {},
      config: {},
    },
  });
  const budgetSchema = JSON.stringify(
    JSON.stringify({
      type: "integer",
      enum: Array.from({ length: 400 }, (_, index) => index),
    }),
  );
  const body = `    body:\n      application/json:\n        schema: ${budgetSchema}\n    responses:\n      204: {description: Empty}\n`;
  for (const limits of [{ maxNodes: 600 }, { maxDocuments: 1 }]) {
    cases.push({
      kind: "schema-resource-limit",
      expectedError: "SCHEMA_DERIVED_LIMIT",
      request: {
        input: {
          uri: "memory://fixtures/legacy.raml",
          content: `${prefix}/one:\n  post:\n${body}/two:\n  post:\n${body}`,
          generated: false,
        },
        files: {},
        config: { limits },
      },
    });
  }
  return cases;
}
