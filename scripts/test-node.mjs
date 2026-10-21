import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import {
  Converter,
  LosslessNumber,
  parseJson,
  stringifyJson,
  object,
} from "../packages/node/index.mjs";
import { transportCases } from "./transport-cases.mjs";
import { draft7Requests, inheritedDraft7Requests } from "./draft7-cases.mjs";
import { wsdlDefaultCases, wsdlMethodCases } from "./wsdl-cases.mjs";
import { raml08Cases } from "./raml-cases.mjs";
import { ramlTemplateCases } from "./raml-template-cases.mjs";
import { postmanSchemaCases } from "./postman-schema-cases.mjs";
import { schemaResourceCases } from "./schema-resource-cases.mjs";
import { ramlSchemaTextCases } from "./raml-schema-text-cases.mjs";
import { ramlXmlCases } from "./raml-xml-cases.mjs";
import { ramlFragmentCases } from "./raml-fragment-cases.mjs";
import { schemaDefinitionCases } from "./schema-definition-cases.mjs";
import { importedOpenapi30Cases } from "./imported-openapi30-cases.mjs";
import Ajv from "ajv";
import Ajv2019 from "ajv/dist/2019.js";
import { draft201909Cases } from "./draft201909-cases.mjs";
import { postmanCredentialCases } from "./postman-auth-cases.mjs";
import { ramlSerializationCases } from "./raml-serialization-cases.mjs";

let checks = 0;
for (const { kind, request } of ramlSerializationCases()) {
  if (kind === "invalid") {
    await assert.rejects(Converter.convert(request), {
      code: "RAML_SERIALIZATION",
    });
    continue;
  }
  const report = await Converter.convert(request);
  assert.equal(report.valid, true);
  const definitions =
    report.document.definitions || report.document.components.schemas;
  if (kind === "xml") {
    assert.equal(report.status, "complete");
    assert.equal(definitions.Item.properties.id.xml.attribute, true);
    assert.equal(definitions.Item.properties.labels.xml.wrapped, true);
    assert.equal(definitions.Item.properties.labels.type, "array");
  } else {
    assert.equal(report.status, "needs-review");
    if (kind === "file") {
      assert.equal(
        definitions.Upload["x-raml-file-byte-length"].maxLength,
        18446744073709551616n,
      );
      assert.equal(definitions.Upload.minLength, undefined);
      assert.deepEqual(definitions.Upload["x-raml-file-types"], [
        "image/png",
        "image/*",
      ]);
    } else {
      assert.equal(definitions.Item["x-raml-xml"].namespace, "relative/path");
    }
  }
}
for (const { kind, request } of postmanCredentialCases()) {
  const report = await Converter.convert(request);
  assert.equal(report.handoffAllowed, false);
  if (kind !== "selected") {
    assert.equal(report.status, "blocked");
    continue;
  }
  assert.equal(report.status, "needs-review");
  assert.ok(
    !JSON.stringify(report.credentialSelectionRequests).includes(
      "credential-secret-never-in-output",
    ),
  );
  assert.ok(
    !report.output.content.includes("credential-secret-never-in-output"),
  );
  const reviewed = await Converter.review({
    ...request,
    config: {
      ...request.config,
      acceptedDecisions: report.decisions.map((decision) => decision.id),
    },
  });
  assert.equal(reviewed.status, "complete");
  assert.equal(reviewed.handoffAllowed, true);
  assert.deepEqual(await Converter.export(reviewed, "json"), reviewed);
}
for (const {
  request,
  pointer,
  accepted,
  rejected,
  blocked,
} of draft201909Cases()) {
  const report = await Converter.convert(request);
  if (blocked) {
    assert.equal(report.status, "blocked");
    assert.equal(report.handoffAllowed, false);
    continue;
  }
  assert.equal(report.valid, true);
  assert.equal(report.status, "complete");
  const ajv = new Ajv2019({ strict: false, validateFormats: false });
  for (const schema of Object.values(report.document.components.schemas)) {
    if (schema.$id) ajv.addSchema(schema, schema.$id);
  }
  ajv.addSchema(report.document, "memory://download/2019.json");
  const validate = ajv.compile({
    $ref: `memory://download/2019.json#${pointer}`,
  });
  for (const value of accepted)
    assert.equal(validate(value), true, JSON.stringify(validate.errors));
  for (const value of rejected)
    assert.equal(validate(value), false, JSON.stringify(value));
}
for (const {
  request,
  expectedError,
  accepted,
  rejected,
  skipInstanceValidation,
} of importedOpenapi30Cases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      assert.ok(error.diagnostics.length);
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  assert.equal(report.valid, true);
  assert.notEqual(report.status, "blocked");
  // AJV does not implement the OpenAPI 3.0 or Draft 4 schema dialect.
  // Rust independently validates the Draft 4 fixture; every transport still
  // compares complete reports for legacy targets and declared Draft 4 inputs.
  if (skipInstanceValidation) continue;
  const ajv = new Ajv({ strict: false, validateSchema: false });
  for (const schema of Object.values(report.document.components.schemas)) {
    if (schema.$id) ajv.addSchema(schema, schema.$id);
  }
  ajv.addSchema(report.document, "memory://download/api.json");
  const validate = ajv.compile({
    $ref: "memory://download/api.json#/paths/~1events/post/requestBody/content/application~1json/schema",
  });
  for (const value of accepted) assert.equal(validate(value), true);
  for (const value of rejected) assert.equal(validate(value), false);
}
for (const { request, name, accepted, rejected } of schemaDefinitionCases()) {
  const report = await Converter.convert(request);
  assert.equal(report.valid, true);
  assert.notEqual(report.status, "blocked");
  const ajv = new Ajv({ strict: false, validateSchema: false });
  for (const root of Object.values(
    report.document.components?.schemas || report.document.definitions,
  )) {
    // These authored fixtures put inline resources directly under $defs.
    // Register them explicitly because an API document is not a schema root.
    for (const schema of [root, ...Object.values(root.$defs || {})]) {
      if (
        schema.id &&
        schema.$schema === "http://json-schema.org/draft-04/schema#"
      ) {
        const compatible = structuredClone(schema);
        delete compatible.id;
        ajv.addSchema(compatible, schema.id);
      } else if (schema.$id) ajv.addSchema(schema, schema.$id);
    }
  }
  const uri = "https://output.example.test/definitions.json";
  ajv.addSchema(report.document, uri);
  const path =
    request.config.targetVersion === "2.0"
      ? "definitions"
      : "components/schemas";
  const validate = ajv.compile({ $ref: `${uri}#/${path}/${name}` });
  for (const value of accepted) assert.equal(validate(value), true);
  for (const value of rejected) assert.equal(validate(value), false);
}
for (const {
  kind,
  request,
  expectedError,
  accepted,
  rejected,
  typeReview,
} of ramlFragmentCases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      assert.ok(error.diagnostics.length);
      assert.equal(
        error.diagnostics[0].location.uri,
        "memory://raml/traits/body.raml",
      );
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  assert.equal(report.valid, true);
  assert.notEqual(report.status, "blocked");
  assert.ok(
    report.decisions.some(
      (decision) => decision.code === "RAML_SCHEMA_FRAGMENT",
    ),
  );
  if (kind === "xml")
    assert.equal(
      report.decisions.some(
        (decision) =>
          decision.code === "RAML_XSD_TYPE_SERIALIZATION" &&
          decision.reviewRequired,
      ),
      typeReview,
    );
  // AJV 8 does not implement Draft 4. Rust's independent jsonschema tests
  // validate these fixtures; every binding still compares their full reports.
  if (
    Object.values(report.document.components.schemas).some(
      (schema) => schema.$schema === "http://json-schema.org/draft-04/schema#",
    )
  )
    continue;
  const ajv = new Ajv({ strict: false, validateSchema: false });
  for (const schema of Object.values(report.document.components.schemas)) {
    if (schema.$id || schema.id) ajv.addSchema(schema, schema.$id || schema.id);
  }
  const uri = "https://output.example.test/api.json";
  ajv.addSchema(report.document, uri);
  const media = kind === "xml" ? "xml" : "json";
  const validate = ajv.compile({
    $ref: `${uri}#/paths/~1items/post/requestBody/content/application~1${media}/schema`,
  });
  for (const value of accepted) assert.equal(validate(value), true);
  for (const value of rejected) assert.equal(validate(value), false);
}
for (const { request, expectedError } of ramlXmlCases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      return true;
    });
  } else {
    const report = await Converter.convert(request);
    assert.equal(report.valid, true);
    assert.notEqual(report.status, "blocked");
    assert.ok(
      report.decisions.some(
        (decision) =>
          decision.code === "RAML_XML_SERIALIZATION" && decision.reviewRequired,
      ),
    );
    assert.ok(!JSON.stringify(report.manifest.files).includes(".octri-xsd-"));
    assert.ok(
      Object.values(report.document.components.schemas).some(
        (schema) =>
          schema.xml?.name === "Item" && schema.xml.namespace === "urn:items",
      ),
    );
  }
}
for (const {
  request,
  expectedError,
  expectedPointer,
  dialect,
} of ramlSchemaTextCases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      if (expectedError === "SOURCE_INVALID") {
        assert.ok(error.diagnostics.length > 0);
        for (const diagnostic of error.diagnostics) {
          assert.equal(
            diagnostic.location.uri,
            "memory://raml/traits/body.raml",
          );
          assert.equal(
            diagnostic.location.pointer,
            expectedPointer ?? "/body/application~1json/type",
          );
          assert.equal(diagnostic.location.line, 3);
        }
      }
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  assert.equal(report.status, "complete");
  assert.equal(report.valid, true);
  assert.equal(report.manifest.files.length, 3);
  assert.ok(!JSON.stringify(report.manifest.files).includes(".octri-schema-"));
  assert.ok(
    Object.values(report.document.components.schemas).some(
      (schema) => schema.$schema === dialect,
    ),
  );
}
for (const { kind, request } of schemaResourceCases()) {
  const report = await Converter.convert(request);
  if (
    ["dynamic", "custom", "missing", "malformed", "invalid-unscoped"].includes(
      kind,
    )
  ) {
    assert.equal(report.status, "blocked", kind);
    assert.equal(report.handoffAllowed, false);
    assert.ok(
      !report.decisions.some(
        (decision) => decision.code === "SCHEMA_RESOURCE_LOCALIZED",
      ),
    );
    if (["missing", "malformed"].includes(kind))
      assert.equal(report.valid, false);
    if (kind === "invalid-unscoped")
      assert.ok(
        report.decisions.some(
          (decision) => decision.code === "SCHEMA_SOURCE_INVALID",
        ),
      );
    continue;
  }
  assert.equal(report.status, "complete", kind);
  assert.equal(report.valid, true, kind);
  assert.equal(report.manifest.files.length, 1);
  assert.ok(
    report.decisions.some(
      (decision) => decision.code === "SCHEMA_RESOURCE_LOCALIZED",
    ),
  );
  const ajv = new Ajv({ strict: false });
  const uri = "https://converted.example.test/api.json";
  ajv.addSchema(report.document, uri);
  const path =
    request.config.targetVersion === "2.0"
      ? "definitions"
      : "components/schemas";
  const check = ajv.compile({ $ref: `${uri}#/${path}/Alias` });
  assert.equal(check(2), true, kind);
  assert.equal(check(6), false, kind);
  assert.equal(check("wrong"), false, kind);
}
for (const { kind, dialect, request, expectedError } of postmanSchemaCases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      if (kind !== "bounded") {
        assert.ok(error.diagnostics.length > 0);
        for (const diagnostic of error.diagnostics) {
          assert.equal(diagnostic.location.uri, request.input.uri);
          assert.equal(diagnostic.location.pointer, "/items/0/body/$type");
          assert.ok(diagnostic.location.line > 1);
        }
      }
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  if (kind === "malformed-selector") {
    assert.equal(report.valid, false);
    assert.equal(report.status, "blocked");
    assert.equal(report.handoffAllowed, false);
    assert.ok(
      !report.decisions.some(
        (decision) => decision.code === "SCHEMA_RESOURCE_LOCALIZED",
      ),
    );
    continue;
  }
  assert.equal(report.valid, true, kind);
  if (kind === "unknown-dialect") {
    assert.equal(report.status, "blocked");
    assert.equal(report.handoffAllowed, false);
    continue;
  }
  assert.equal(report.status, "needs-review", kind);
  assert.equal(report.document.openapi, "3.2.1");
  assert.ok(!JSON.stringify(report.manifest.files).includes(".octri-schema-"));
  assert.equal(report.manifest.files.length, 2);
  const schemas = Object.values(report.document.components.schemas);
  if (dialect) assert.ok(schemas.some((schema) => schema.$schema === dialect));
  const annotation = report.decisions.find(
    (decision) => decision.code === "POSTMAN_AUTHORED_TYPE",
  );
  assert.equal(annotation.source.pointer, "/items/0/body/$type");
  assert.ok(annotation.source.offset > 0);
}
for (const { kind, request, expectedError } of ramlTemplateCases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      assert.ok(
        error.diagnostics.some(
          (diagnostic) =>
            diagnostic.location.uri === "memory://templates/traits/body.raml" &&
            diagnostic.location.pointer === "/body/application~1json/schema" &&
            diagnostic.location.line === 3,
        ),
      );
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  assert.equal(report.valid, true, kind);
  assert.equal(report.status, "complete", kind);
  assert.equal(report.document.openapi, "3.2.1");
  assert.ok(!JSON.stringify(report.manifest.files).includes(".octri-schema-"));
  if (kind === "array-merge") {
    assert.deepEqual(
      report.document.paths["/items"].get.parameters[0].schema.enum,
      Array.from({ length: 192 }, (_, index) => index),
    );
  } else if (kind === "parameterized") {
    assert.notDeepEqual(
      report.document.paths["/text"].post.requestBody,
      report.document.paths["/number"].post.requestBody,
    );
  } else {
    assert.ok(
      report.document.paths["/items"].post.requestBody.content[
        "application/json"
      ].schema,
    );
    if (kind === "included")
      assert.ok(
        report.manifest.files.some((file) =>
          file.uri.endsWith("traits/schemas/value.json"),
        ),
      );
    if (kind === "optional-inherited")
      assert.equal(report.document.paths["/absent"]?.post, undefined);
  }
}
for (const { kind, request, expectedError, expectedPointer } of raml08Cases()) {
  if (expectedError) {
    await assert.rejects(Converter.convert(request), (error) => {
      assert.equal(error.code, expectedError);
      if (expectedPointer) {
        assert.ok(error.diagnostics.length > 0);
        for (const diagnostic of error.diagnostics) {
          assert.equal(diagnostic.location.uri, request.input.uri);
          assert.equal(diagnostic.location.pointer, expectedPointer);
          assert.equal(diagnostic.location.line, 9);
        }
      }
      return true;
    });
    continue;
  }
  const report = await Converter.convert(request);
  if (kind === "declaration-origin-collision") {
    assert.equal(report.status, "complete");
    assert.equal(report.valid, true);
    const declarations = report.decisions.filter(
      (decision) => decision.code === "RAML_LEGACY_DECLARATION",
    );
    assert.deepEqual(
      declarations.map((decision) => decision.source.pointer),
      ["/traits/0/0", "/traits/0/queryParameters"],
    );
    assert.equal(
      report.document.paths["/items"].get.parameters[0].name,
      "used",
    );
    continue;
  }
  if (kind === "schema-downgrade" || kind === "schema-unknown-dialect") {
    assert.equal(report.status, "blocked", kind);
    assert.equal(report.handoffAllowed, false, kind);
    continue;
  }
  if (kind === "unknown-body-examples") {
    assert.equal(report.valid, true);
    assert.notEqual(report.status, "complete");
    assert.ok(
      report.decisions.some((decision) => decision.code === "RAML_BODY_FACET"),
    );
    const media =
      report.document.paths["/items"].post.requestBody.content[
        "application/json"
      ];
    assert.deepEqual(media.example, { value: 7, extra: true });
    assert.equal(media.examples, undefined);
    continue;
  }
  assert.equal(report.valid, true, kind);
  assert.equal(report.status, "complete", kind);
  assert.equal(report.document.openapi, request.config.targetVersion);
  assert.equal(report.document.servers[0].url, "https://api.example.test/0.8");
  if (kind === "transport") {
    const parameters = report.document.paths["/items"].get.parameters;
    assert.ok(parameters.every((parameter) => parameter.required === false));
    const tags = parameters.find((parameter) => parameter.name === "tags");
    assert.equal(tags.schema.type, "array");
    assert.equal(tags.explode, true);
  } else {
    const resources = Object.values(report.document.components.schemas);
    const resource = resources.find(
      (schema) => schema.id === "https://schemas.example.test/payload.json",
    );
    assert.equal(resource.$schema, "http://json-schema.org/draft-04/schema#");
    assert.equal(resource.definitions.Value.exclusiveMinimum, true);
    assert.ok(Array.isArray(resource.properties.pair.items));
    assert.equal(resource.properties.pair.additionalItems, false);
    assert.ok(
      !JSON.stringify(report.manifest.files).includes(".octri-schema-"),
    );
  }
}
for (const { mode, request } of wsdlDefaultCases()) {
  const report = await Converter.convert(request);
  assert.equal(report.valid, true);
  assert.notEqual(report.status, "blocked");
  const path = mode === "override" ? "/v1/read" : "/v1/";
  const method =
    mode === "override"
      ? "delete"
      : mode === "soap-post" || mode === "soap-headers" || mode === "soap-fault"
        ? "post"
        : "get";
  const operation = report.document.paths[path][method];
  assert.ok(operation.operationId);
  assert.ok(operation.security[0]);
  if (mode === "soap-get") assert.equal(operation.requestBody, undefined);
  if (mode === "soap-post")
    assert.equal(
      operation.requestBody.content["application/soap+xml"].schema[
        "x-soap-envelope"
      ],
      true,
    );
  if (mode === "soap-headers")
    assert.deepEqual(
      operation.requestBody.content["application/soap+xml"].schema.properties
        .Header.required,
      ["{urn:defaults}Auth"],
    );
  if (mode === "soap-fault")
    assert.deepEqual(
      operation.responses["500"].content["application/soap+xml"].schema.anyOf[0]
        .properties.Header.required,
      ["{urn:defaults}Auth"],
    );
}
for (const { method, targetVersion, request } of wsdlMethodCases()) {
  const report = await Converter.convert(request);
  if (method !== "GET" && targetVersion === "3.1.2") {
    assert.equal(report.status, "blocked");
    assert.equal(report.handoffAllowed, false);
  } else {
    assert.equal(report.valid, true);
    assert.notEqual(report.status, "blocked");
    const item = report.document.paths["/v1/call"];
    const operation =
      method === "GET"
        ? item.get
        : method === "QUERY"
          ? item.query
          : item.additionalOperations[method];
    assert.ok(operation.operationId);
    if (method === "get") assert.equal(item.get, undefined);
  }
}
for (const request of inheritedDraft7Requests()) {
  const source = JSON.parse(request.input.content);
  const report = await Converter.convert(request);
  const modernOverride = source.components.schemas.Alias.$schema !== undefined;
  assert.equal(report.status, modernOverride ? "blocked" : "complete");
  assert.equal(report.handoffAllowed, !modernOverride);
  assert.deepEqual(
    report.document,
    source,
    "Unbundled output retains original schema fields",
  );
}
for (const envelope of transportCases()) {
  const input = stringifyJson(envelope);
  const expected = spawnSync("target/release/octri-spec-convert", ["execute"], {
    input,
    encoding: "utf8",
    timeout: 30000,
    maxBuffer: 128 * 1024 * 1024,
  });
  assert.ok([0, 2].includes(expected.status), expected.stderr);
  assert.deepEqual(
    parseJson(await Converter.executeJson(input)),
    parseJson(expected.stdout),
    envelope.action,
  );
  checks++;
}
const exact = object({
  openapi: "3.1.2",
  info: { title: "Node", version: "1" },
  paths: {},
  "x-decimal": new LosslessNumber("0.100000000000000005"),
  "x-integer": 18446744073709551616n,
});
const report = await Converter.convert({ input: exact });
assert.equal(report.document["x-decimal"].token, "0.100000000000000005");
assert.equal(report.document["x-integer"], 18446744073709551616n);
assert.deepEqual(await Converter.export(report, "json"), report);
const require = createRequire(import.meta.url);
const common = require("../packages/node/index.cjs");
assert.equal(common.LosslessNumber, LosslessNumber);
assert.deepEqual(await common.convert({ input: exact }), report);
for (const request of draft7Requests()) {
  const report = await Converter.convert(request);
  assert.equal(report.status, "complete");
  assert.equal(report.handoffAllowed, true);
  const document = report.document;
  const ajv = new Ajv({ strict: false, validateFormats: false });
  for (const schema of Object.values(document.components.schemas)) {
    if (schema.$id) ajv.addSchema(schema, schema.$id);
  }
  ajv.addSchema(document, "memory://download/api.json");
  const validate = ajv.compile({
    $ref: "memory://download/api.json#/paths/~1events/post/requestBody/content/application~1json/schema",
  });
  for (const instance of [
    {},
    { pair: [] },
    { pair: ["ab", 2], code: "ab" },
    { card: 1, billing: "address" },
    { flag: true, code: "ab" },
  ]) {
    assert.equal(validate(instance), true, JSON.stringify(validate.errors));
  }
  for (const instance of [
    { pair: ["a"] },
    { pair: ["ab", 2, 3] },
    { code: 1 },
    { card: 1 },
    { flag: true },
  ]) {
    assert.equal(validate(instance), false, JSON.stringify(instance));
  }
}
console.log(
  `Node ESM/CommonJS: ${checks} complete Rust results plus exact object, verified export and package facades passed`,
);
