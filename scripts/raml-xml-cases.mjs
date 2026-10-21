/** Authored XML contracts; every transport must preserve complete Rust results. */
export function ramlXmlCases() {
  const schema = `<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:c="urn:counts" targetNamespace="urn:items" elementFormDefault="qualified"><x:import namespace="urn:counts" schemaLocation="count.xsd"/><x:element name="Item"><x:complexType><x:all><x:element name="count" type="c:Count"/><x:element name="label" type="x:string" minOccurs="0"/></x:all></x:complexType></x:element></x:schema>`;
  const count = `<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:counts"><x:simpleType name="Count"><x:restriction base="x:integer"><x:minInclusive value="2"/></x:restriction></x:simpleType></x:schema>`;
  const cases = [];
  for (const version of ["0.8", "1.0"]) {
    const facet = version === "0.8" ? "schema" : "type";
    for (const included of [false, true]) {
      const request = {
        input: {
          uri: "memory://raml/api.raml",
          generated: false,
          content: `#%RAML ${version}\ntitle: XML bodies\nversion: v1\nbaseUri: https://api.example.test\ntraits:\n${version === "0.8" ? "  - payload:" : "  payload:"} !include traits/body.raml\n/items:\n  post:\n    is: [payload]\n    responses:\n      204: {description: Empty}\n`,
        },
        files: {
          "traits/body.raml": `body:\n  application/xml:\n    ${facet}: ${included ? "!include input.xsd" : JSON.stringify(schema)}\n`,
          "traits/count.xsd": count,
          ...(included ? { "traits/input.xsd": schema } : {}),
        },
        config: { targetVersion: "3.2.1" },
      };
      cases.push({ kind: included ? "included" : "inline", request });
      const invalidMedia = structuredClone(request);
      invalidMedia.files["traits/body.raml"] = invalidMedia.files[
        "traits/body.raml"
      ].replace("application/xml", "application/json");
      cases.push({
        kind: "media",
        expectedError: "RAML_SCHEMA_PLACEMENT",
        request: invalidMedia,
      });
    }
  }
  for (const [text, expectedError] of [
    ["<broken>", "XML_SYNTAX"],
    [
      '<!DOCTYPE schema SYSTEM "https://attacker.test/schema.dtd"><schema/>',
      "XML_DTD_DISABLED",
    ],
  ]) {
    const request = structuredClone(cases[0].request);
    request.files["traits/body.raml"] =
      `body:\n  application/xml:\n    schema: ${JSON.stringify(text)}\n`;
    cases.push({ kind: "invalid", expectedError, request });
  }
  const xml =
    '<x:schema xmlns:x="http://www.w3.org/2001/XMLSchema"><x:element name="Item" type="x:string"/></x:schema>';
  for (const original of cases.filter(
    ({ kind }) => kind === "inline" || kind === "included",
  )) {
    const bom = structuredClone(original);
    bom.kind = `bom-${bom.kind}`;
    for (const [path, content] of Object.entries(bom.request.files)) {
      bom.request.files[path] = path.endsWith(".xsd")
        ? "\uFEFF" + content
        : content.replace(
            /^(\s*)(type|schema): (".*")$/gm,
            (_, indent, facet, encoded) =>
              `${indent}${facet}: ${JSON.stringify("\uFEFF" + JSON.parse(encoded))}`,
          );
    }
    cases.push(bom);
  }
  for (const limit of ["documents", "bytes", "nodes"]) {
    const description = "x".repeat(1000);
    const schema =
      limit === "nodes"
        ? { type: "integer", enum: Array.from({ length: 100 }, (_, i) => i) }
        : { type: "string", description };
    const xmlSchema =
      limit === "bytes"
        ? xml.replace(
            "<x:element",
            `<x:annotation><x:documentation>${description}</x:documentation></x:annotation><x:element`,
          )
        : xml;
    const aliases = limit === "bytes" ? "  JsonB: *json\n  XmlB: *xml\n" : "";
    const content = `#%RAML 1.0\ntitle: Mixed\nversion: v1\nbaseUri: https://api.example.test\ntypes:\n  Json: &json ${JSON.stringify(JSON.stringify(schema))}\n  Xml: &xml ${JSON.stringify(xmlSchema)}\n${aliases}/items:\n  post:\n    body:\n      application/xml: Xml\n    responses:\n      204: {description: Empty}\n`;
    cases.push({
      kind: `mixed-${limit}`,
      expectedError:
        limit === "bytes" ? "SOURCE_INVALID" : "SCHEMA_DERIVED_LIMIT",
      request: {
        input: { uri: "memory://mixed/api.raml", generated: false, content },
        files: {},
        config: {
          targetVersion: "3.2.1",
          limits:
            limit === "documents"
              ? { maxDocuments: 1 }
              : limit === "bytes"
                ? {
                    maxTotalBytes:
                      new TextEncoder().encode(content).length + 32,
                  }
                : { maxNodes: 105 },
        },
      },
    });
  }
  return cases;
}
