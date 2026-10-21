/** Authored 2019-09 constraints; shared by every transport and independent AJV checks. */
export function draft201909Cases() {
  const dialect = "https://json-schema.org/draft/2019-09/schema";
  const tuple = {
    $schema: dialect,
    $id: "https://schemas.example.test/pair",
    type: "array",
    $defs: { Code: { type: "string", minLength: 2 } },
    items: [{ $ref: "#/$defs/Code" }, { type: "integer" }],
    additionalItems: false,
    prefixItems: [{ $ref: "inert.json" }],
  };
  const tree = {
    $schema: dialect,
    $id: "https://schemas.example.test/strict-tree",
    $recursiveAnchor: true,
    $ref: "#/$defs/tree",
    required: ["name"],
    $defs: {
      tree: {
        $id: "https://schemas.example.test/tree",
        $recursiveAnchor: true,
        type: "object",
        properties: {
          name: { type: "string" },
          children: { type: "array", items: { $recursiveRef: "#" } },
        },
      },
    },
  };
  const make = (schema, targetVersion = "3.2.1") => ({
    input: {
      uri: "memory://schemas/2019.json",
      generated: false,
      content: JSON.stringify({
        openapi: "3.1.2",
        info: { title: "2019 schemas", version: "1" },
        paths: {},
        components: { schemas: { Payload: schema } },
      }),
    },
    config: { targetVersion },
  });
  const accepted = [["ok", 2], []];
  const rejected = [
    ["x", 2],
    ["ok", "bad"],
    ["ok", 2, 3],
  ];
  return [
    {
      request: make(tuple),
      pointer: "/components/schemas/Payload",
      accepted,
      rejected,
    },
    {
      request: make(tree),
      pointer: "/components/schemas/Payload",
      accepted: [{ name: "root", children: [{ name: "child" }] }],
      rejected: [{ name: "root", children: [{}] }],
    },
    { request: make(tuple, "3.0.4"), blocked: true },
    {
      request: {
        input: {
          uri: "memory://schemas/events.json",
          generated: false,
          content: JSON.stringify({
            asyncapi: "3.0.0",
            info: { title: "2019 events", version: "1" },
            channels: {
              events: {
                messages: {
                  event: {
                    payload: {
                      schemaFormat:
                        "application/schema+json;version=draft-2019-09",
                      schema: tuple,
                    },
                  },
                },
              },
            },
            operations: {
              receive: {
                action: "receive",
                channel: { $ref: "#/channels/events" },
              },
            },
          }),
        },
        config: {
          targetVersion: "3.2.1",
          httpMappings: {
            receive: {
              method: "POST",
              path: "/events",
              server: "https://api.example.test",
              responseStatus: "202",
              security: [],
              payloadLocation: "request",
            },
          },
        },
      },
      pointer:
        "/paths/~1events/post/requestBody/content/application~1json/schema",
      accepted,
      rejected,
    },
  ];
}
