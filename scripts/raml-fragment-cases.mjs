/** Schema selectors exercise physical include sites in every binding. */
export function ramlFragmentCases() {
  const xml =
    '<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:items"><x:element name="Text" type="x:string"/><x:element name="Count" type="x:integer"/><x:complexType name="Payload"><x:all><x:element name="value" type="x:boolean"/></x:all></x:complexType></x:schema>';
  const make = (path, source, media = "json") => ({
    input: {
      uri: "memory://raml/api.raml",
      generated: false,
      content:
        "#%RAML 1.0\ntitle: Selected\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n  payload: !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n",
    },
    files: {
      "traits/body.raml": `body:\n  application/${media}:\n    type: !include ${path}\n`,
      [`traits/schema.${media === "xml" ? "xsd" : "json"}`]: source,
    },
    config: { targetVersion: "3.2.1" },
  });
  const cases = [];
  for (const [name, accepted, rejected] of [
    ["Text", "ok", 12],
    ["Count", 12, "wrong"],
    ["Payload", { value: true }, { value: "wrong" }],
  ])
    cases.push({
      kind: "xml",
      request: make(`schema.xsd#${name}`, xml, "xml"),
      accepted: [accepted],
      rejected: [rejected],
      typeReview: name === "Payload",
    });
  for (const selector of ["Missing", "Text/child", "Text%XX", "%FF"]) {
    cases.push({
      kind: "invalid",
      expectedError: "RAML_SCHEMA_FRAGMENT",
      request: make(`schema.xsd#${selector}`, xml, "xml"),
    });
  }
  for (const [dialect, definitions] of [
    ["http://json-schema.org/draft-04/schema#", "definitions"],
    ["http://json-schema.org/draft-07/schema#", "definitions"],
    ["https://json-schema.org/draft/2020-12/schema", "$defs"],
  ]) {
    const source = JSON.stringify({
      $schema: dialect,
      [definitions]: {
        Payload: {
          type: "object",
          properties: { value: { $ref: `#/${definitions}/Count` } },
          required: ["value"],
        },
        Count: { type: "integer", minimum: 2 },
      },
    });
    cases.push({
      kind: "json",
      request: make(`schema.json#/${definitions}/Payload`, source),
      accepted: [{ value: 2 }],
      rejected: [{ value: 1 }, "wrong"],
    });
  }
  cases.push({
    kind: "json",
    request: make("schema.json#/$defs/Never", '{"$defs":{"Never":false}}'),
    accepted: [],
    rejected: [null, "wrong"],
  });
  cases.push({
    kind: "json",
    request: make(
      "schema.json#/%24defs/%E7%8C%AB~1~0",
      '\uFEFF{"$defs":{"猫/~":{"const":"Code"}}}',
    ),
    accepted: ["Code"],
    rejected: ["wrong"],
  });
  for (const selector of [
    "Anchor",
    "/missing",
    "/properties/value/type",
    "/bad~2",
    "/%FF",
    "/%XX",
  ])
    cases.push({
      kind: "invalid",
      expectedError: "RAML_SCHEMA_FRAGMENT",
      request: make(
        `schema.json#${selector}`,
        '{"properties":{"value":{"type":"string"}}}',
      ),
    });
  cases.push({
    kind: "invalid",
    expectedError: "RAML_SCHEMA_FRAGMENT",
    request: make(
      "schema.json#/$defs/Payload",
      '{"$schema":"https://example.test/custom","$defs":{"Payload":{"type":"string"}}}',
    ),
  });
  const legacy = structuredClone(cases[0]);
  legacy.request.input.content = legacy.request.input.content
    .replace("#%RAML 1.0", "#%RAML 0.8")
    .replace("  payload:", "  - payload:");
  legacy.request.files["traits/body.raml"] = legacy.request.files[
    "traits/body.raml"
  ].replace("    type:", "    schema:");
  cases.push(legacy);
  for (const [media, path, source] of [
    [
      "json",
      "schema.json#/$defs/Body",
      '{"$defs":{"Body":{"const":"<<value>>"}}}',
    ],
    [
      "xml",
      "schema.xsd#Body",
      '<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Body"><x:simpleType><x:restriction base="x:string"><x:enumeration value="<<value>>"/></x:restriction></x:simpleType></x:element></x:schema>',
    ],
  ]) {
    const request = make(path, source, media);
    request.input.content = request.input.content.replace(
      "is: [payload]",
      "is: [{payload: {value: done}}]",
    );
    cases.push({
      kind: media,
      request,
      accepted: ["done"],
      rejected: ["<<value>>", "wrong"],
      typeReview: false,
    });
  }
  return cases;
}
