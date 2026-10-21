/** Deferred qualification fixtures for the Rust serialization-facet adapter. */
export function ramlSerializationCases() {
  const prefix =
    "#%RAML 1.0\ntitle: Serialization\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n";
  const xml =
    "  Identifier: string\n  Labels: string[]\n  Item:\n    type: object\n    xml: {name: item, namespace: 'https://example.test/schema', prefix: s}\n    properties:\n      id:\n        type: Identifier\n        xml: {attribute: true}\n      labels:\n        type: Labels\n        xml: {wrapped: true}\n";
  const file =
    "  Upload:\n    type: file\n    fileTypes: [image/png, 'image/*']\n    minLength: 2\n    maxLength: 18446744073709551616\n";
  return [
    ["xml", xml, "3.0.4"],
    ["xml", xml, "3.2.1"],
    ["file", file, "2.0"],
    ["file", file, "3.1.2"],
    [
      "review",
      "  Item:\n    type: object\n    xml: {wrapped: true, namespace: relative/path}\n",
      "3.2.1",
    ],
    ["invalid", "  Item: {type: object, xml: {attribute: true}}\n", "3.1.2"],
  ].map(([kind, types, targetVersion]) => ({
    kind,
    request: {
      input: {
        uri: "memory://serialization/api.raml",
        content: prefix + types,
        generated: false,
      },
      files: {},
      config: { targetVersion },
    },
  }));
}
