/** Owning OpenAPI 3.0 schemas migrate before foreign HTTP payload validation. */
export function importedOpenapi30Cases() {
  const make = (schema, targetVersion = "3.2.1", files = {}) => ({
    input: {
      uri: "memory://imports/events.json",
      content: JSON.stringify({
        asyncapi: "2.6.0",
        info: { title: "Events", version: "1" },
        channels: {
          events: {
            publish: {
              message: {
                payload: {
                  $ref: "./api.json#/components/schemas/Value",
                },
              },
            },
          },
        },
      }),
    },
    files: {
      "memory://imports/api.json": JSON.stringify({
        openapi: "3.0.4",
        info: { title: "Models", version: "1" },
        paths: {},
        components: { schemas: { Value: schema, Base: { type: "number" } } },
      }),
      ...Object.fromEntries(
        Object.entries(files).map(([uri, value]) => [
          /^[a-z][a-z0-9+.-]*:/i.test(uri) ? uri : `memory://imports/${uri}`,
          JSON.stringify(value),
        ]),
      ),
    },
    config: {
      targetVersion,
      ...(targetVersion === "2.0"
        ? {
            baseUrl: "https://example.test",
            dropUnsupported: ["/paths/~1events/post/servers"],
          }
        : {}),
      httpMappings: {
        "publish events": {
          method: "POST",
          path: "/events",
          server: "https://example.test",
          responseStatus: "202",
          security: [],
          payloadLocation: "request",
        },
      },
    },
  });
  const cases = [];
  for (const target of [
    "2.0",
    "3.0.0",
    "3.0.1",
    "3.0.2",
    "3.0.3",
    "3.0.4",
    "3.1.0",
    "3.1.1",
    "3.1.2",
    "3.2.0",
    "3.2.1",
  ])
    cases.push({
      request: make(
        { type: "number", minimum: 1, exclusiveMinimum: true },
        target,
      ),
      accepted: [2],
      rejected: [1, "wrong"],
      skipInstanceValidation: target === "2.0" || target.startsWith("3.0."),
    });
  for (const target of ["3.1.2", "3.2.1"]) {
    cases.push({
      request: make(
        { type: "number", nullable: true, minimum: 1, exclusiveMinimum: true },
        target,
      ),
      accepted: [2, null],
      rejected: [1, "wrong"],
    });
    cases.push({
      request: make({ $ref: "./value.json" }, target, {
        "value.json": {
          type: "object",
          properties: {
            count: {
              type: "number",
              nullable: true,
              minimum: 1,
              exclusiveMinimum: true,
            },
            next: { $ref: "./value.json" },
          },
        },
      }),
      accepted: [{ count: null, next: { count: 2 } }],
      rejected: [{ count: 1 }, { next: { count: 1 } }],
    });
  }
  cases.push({
    request: make({ $ref: "./schema.json" }, "3.2.1", {
      "schema.json": {
        $schema: "http://json-schema.org/draft-04/schema#",
        type: "number",
        nullable: true,
        minimum: 1,
        exclusiveMinimum: true,
      },
    }),
    accepted: [2],
    rejected: [1, null],
    skipInstanceValidation: true,
  });
  cases.push({
    request: make({ $ref: "./alias.json" }, "3.2.1", {
      "alias.json": { $ref: "./number.json", type: "string", minimum: 1000 },
      "number.json": {
        type: "number",
        nullable: true,
        minimum: 1,
        exclusiveMinimum: true,
      },
    }),
    accepted: [2, null],
    rejected: [1, "wrong"],
  });
  for (const schema of [
    { type: "number", nullable: "wrong" },
    { type: "number", exclusiveMinimum: 2 },
  ])
    cases.push({
      request: make(schema),
      expectedError: "IMPORTED_OPENAPI30_INVALID",
    });
  cases.push({
    request: make({ $ref: "./alias.json" }, "3.2.1", {
      "alias.json": { $ref: "./number.json", $id: "memory://wrong/" },
      "number.json": { type: "number", minimum: 2 },
    }),
    accepted: [2, 3],
    rejected: [1, "wrong"],
  });
  cases.push({
    request: make({
      $ref: "#/components/schemas/Base",
      $id: "memory://wrong/",
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $dynamicRef: "./missing.json",
      type: "string",
    }),
    accepted: [2],
    rejected: ["wrong", null],
  });
  cases.push({
    request: make({ $ref: "./external.json#/Value" }, "3.2.1", {
      "external.json": {
        Value: {
          $schema: "https://json-schema.org/draft/2020-12/schema",
          $id: "https://schemas.example.test/value.json",
          $ref: "./number.json",
          maximum: 8,
        },
      },
      "https://schemas.example.test/number.json": {
        type: "number",
        exclusiveMinimum: 2,
      },
    }),
    accepted: [3, 8],
    rejected: [2, 9, null],
  });
  const conflict = make({ $ref: "./plain.json" }, "3.2.1", {
    "plain.json": { type: "number", nullable: true },
  });
  const source = JSON.parse(conflict.input.content);
  source.channels.direct = {
    publish: { message: { payload: { $ref: "./plain.json" } } },
  };
  conflict.input.content = JSON.stringify(source);
  conflict.config.httpMappings["publish direct"] = {
    ...conflict.config.httpMappings["publish events"],
    path: "/direct",
  };
  cases.push({ request: conflict, expectedError: "SCHEMA_DIALECT_CONFLICT" });
  return cases;
}
