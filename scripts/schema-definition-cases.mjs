/** Legacy definition migration and parser inlining share the transport corpus. */
export function schemaDefinitionCases() {
  const cases = [];
  for (const targetVersion of ["2.0", "3.0.4", "3.2.1"]) {
    const schemas = {
      Root: {
        type: "object",
        properties: {
          count: { $ref: "#/components/schemas/Root/$defs/Count" },
        },
        required: ["count"],
        $defs: { Count: { type: "integer", minimum: 2 } },
      },
    };
    cases.push({
      targetVersion,
      schemas,
      name: "Root",
      accepted: [{ count: 2 }],
      rejected: [{ count: 1 }, { count: "wrong" }],
    });
  }
  const schemas = {
    Payload: { $ref: "#/components/schemas/Z/definitions/Count", minimum: 5 },
    Z: {
      $schema: "http://json-schema.org/draft-04/schema#",
      $ref: "#/components/schemas/Z/definitions/Text",
      definitions: {
        Count: { type: "integer", minimum: 0 },
        Text: { type: "string" },
      },
    },
  };
  for (const targetVersion of ["2.0", "3.0.4"])
    cases.push({
      targetVersion,
      schemas: {
        Root: {
          type: "integer",
          minimum: 2,
          $defs: {},
          definitions: {},
        },
      },
      name: "Root",
      accepted: [2],
      rejected: [1, "wrong"],
    });
  for (const targetVersion of ["2.0", "3.0.4", "3.2.1"])
    cases.push({
      targetVersion,
      schemas,
      name: "Payload",
      accepted: [5],
      rejected: [4, "wrong"],
    });
  return cases.map(({ targetVersion, schemas, ...checks }) => ({
    ...checks,
    request: {
      input: {
        uri: "memory://schemas/api.json",
        generated: false,
        content: JSON.stringify({
          openapi: "3.1.2",
          info: { title: "Schema migration", version: "1" },
          paths: {},
          components: { schemas },
        }),
      },
      config: { targetVersion },
    },
  }));
}
