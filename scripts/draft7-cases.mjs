/** Source-authored Draft 7 contracts shared by native and WASM consumers. */
export function draft7Requests() {
  const requests = [];
  for (const version of ["2.6.0", "3.0.0"]) {
    const payload = {
      type: "object",
      properties: {
        pair: {
          type: "array",
          items: [{ $ref: "#Code" }, { type: "integer" }],
          additionalItems: false,
        },
        code: { $ref: "#Code", type: "integer" },
      },
      dependencies: {
        card: ["billing"],
        flag: { required: ["code"] },
      },
      example: { $ref: "literal-example" },
    };
    const source = {
      asyncapi: version,
      info: { title: "Draft 7 event contract", version: "1" },
      components: {
        schemas: { Code: { $id: "#Code", type: "string", minLength: 2 } },
      },
      channels: version.startsWith("2")
        ? { events: { publish: { message: { payload } } } }
        : { events: { messages: { event: { payload } } } },
    };
    if (version.startsWith("3")) {
      source.operations = {
        receive: { action: "receive", channel: { $ref: "#/channels/events" } },
      };
    }
    const key = version.startsWith("2") ? "publish events" : "receive";
    for (const targetVersion of ["3.1.2", "3.2.1"]) {
      requests.push({
        input: {
          uri: "memory://fixtures/draft7.json",
          content: JSON.stringify(source),
          generated: false,
        },
        files: {},
        config: {
          targetVersion,
          httpMappings: {
            [key]: {
              method: "POST",
              path: "/events",
              server: "https://api.example.com",
              responseStatus: "202",
              security: [],
              payloadLocation: "request",
            },
          },
        },
      });
    }
  }
  return requests;
}

/** Global defaults also govern unbundled schemas; local selectors take priority. */
export function inheritedDraft7Requests() {
  return ["3.1.2", "3.2.1"].flatMap((version) =>
    [false, true].map((modernOverride) => ({
      input: {
        uri: "memory://fixtures/inherited.json",
        content: JSON.stringify({
          openapi: version,
          jsonSchemaDialect: "http://json-schema.org/draft-07/schema#",
          info: { title: "Inherited dialect", version: "1" },
          paths: {},
          components: {
            schemas: {
              Number: { type: "number" },
              Alias: {
                ...(modernOverride
                  ? { $schema: "https://json-schema.org/draft/2020-12/schema" }
                  : {}),
                $ref: "#/components/schemas/Number",
                properties: {
                  field: {
                    $schema: "https://dialects.example/custom",
                    xml: { nodeType: "attribute" },
                  },
                },
              },
            },
          },
        }),
        generated: false,
      },
      files: {},
      config: { bundle: false },
    })),
  );
}
