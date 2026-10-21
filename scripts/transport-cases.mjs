/** Hand-authored consumer fixtures. Rust supplies the reference engine results. */
import { launchCases } from "./launch-cases.mjs";
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
import {
  postmanAuthCases,
  postmanCredentialCases,
} from "./postman-auth-cases.mjs";
import { draft201909Cases } from "./draft201909-cases.mjs";
import { ramlSerializationCases } from "./raml-serialization-cases.mjs";
import { readFileSync } from "node:fs";

export function transportCases() {
  const make = (content, name = "source.json", config = {}, files = {}) => ({
    input: {
      uri: `memory://fixtures/${name}`,
      content: typeof content === "string" ? content : JSON.stringify(content),
      generated: false,
    },
    config,
    files,
  });
  const modern = {
    openapi: "3.1.2",
    info: { title: "Pets 猫 Café", version: "1" },
    paths: {
      "/pets": {
        get: {
          operationId: "listPets",
          responses: {
            200: {
              description: "Pets",
              content: {
                "application/json": {
                  schema: { type: "array", items: { type: "string" } },
                },
              },
            },
          },
        },
      },
    },
  };
  const requests = [
    make(modern),
    make({
      openapi: "3.2.1",
      info: { title: "Case-sensitive methods", version: "1" },
      paths: {
        "/events": {
          additionalOperations: {
            COPY: {
              operationId: "copyUpper",
              description: "Uppercase custom method",
              "x-interfacer-namespace": "events",
              responses: { 204: { description: "Copied" } },
            },
            copy: {
              operationId: "copyLower",
              description: "Lowercase custom method",
              "x-evidence": { $ref: "literal-data" },
              responses: { 204: { description: "Copied" } },
            },
          },
        },
      },
    }),
    make(
      '# Query contract\n"""Public API"""\ntype Query { ping: String }',
      "pasted.yaml",
      { inputFormat: "graphql" },
    ),
    make({
      swagger: "2.0",
      info: { title: "Legacy", version: "1" },
      host: "api.example.com",
      schemes: ["https"],
      paths: {
        "/pets": { get: { responses: { 204: { description: "Empty" } } } },
      },
    }),
    make({
      info: {
        name: "Requests",
        schema:
          "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
      },
      auth: { type: "noauth" },
      item: [
        {
          name: "List pets",
          request: {
            method: "GET",
            url: "https://api.example.com/pets?tag=a&tag=b",
          },
        },
      ],
    }),
    make(
      "curl 'https://api.example.com/pets' --json '{\"name\":\"猫\"}'",
      "request.curl",
    ),
    make({
      type: "collection.insomnia.rest/5.0",
      name: "Requests",
      collection: [
        {
          name: "List",
          method: "GET",
          url: "https://api.example.com/pets",
          authentication: { type: "none" },
        },
      ],
    }),
    make({
      log: {
        version: "1.2",
        entries: [
          {
            request: {
              method: "GET",
              url: "https://api.example.com/pets",
              headers: [],
            },
            response: {
              status: 204,
              statusText: "Empty",
              headers: [],
              content: { mimeType: "text/plain", text: "" },
            },
          },
        ],
      },
    }),
    make("type Query { pet(id: ID!): String }", "schema.graphql", {
      httpMappings: {
        "Query.pet": {
          method: "GET",
          path: "/pets/{id}",
          server: "https://api.example.com",
          responseStatus: "200",
          parameterBindings: { id: { location: "path", name: "id" } },
          security: [],
        },
      },
    }),
    make(
      'syntax = "proto3"; message Input {} message Output { uint64 id = 1; } service Pets { rpc List(Input) returns (Output); }',
      "service.proto",
      {
        httpMappings: {
          "Pets.List": {
            method: "GET",
            path: "/pets",
            server: "https://api.example.com",
            responseStatus: "200",
            security: [],
          },
        },
      },
    ),
    make(
      {
        asyncapi: "3.0.0",
        info: { title: "Events", version: "1" },
        channels: {
          pets: {
            address: "pets",
            messages: { pet: { payload: { type: "string" } } },
          },
        },
        operations: {
          consume: { action: "receive", channel: { $ref: "#/channels/pets" } },
        },
      },
      "asyncapi.json",
      {
        httpMappings: {
          consume: {
            method: "POST",
            path: "/pets",
            server: "https://api.example.com",
            responseStatus: "202",
            security: [],
            payloadLocation: "request",
          },
        },
      },
    ),
    make(
      {
        openapi: "3.1.2",
        info: { title: "External", version: "1" },
        paths: {},
        components: { schemas: { Pet: { $ref: "pet.json" } } },
      },
      "root.json",
      { bundle: true },
      {
        "pet.json":
          '{"type":"object","properties":{"é":{"type":"string"},"é":{"type":"integer"}}}',
      },
    ),
  ];
  const exactMethods = ["GET", "QUERY", "COPY", "copy", "get", "~COPY"];
  requests.push(
    make(
      exactMethods
        .map((method) => `curl -X '${method}' https://api.example.com/pets`)
        .join("\n"),
      "methods.curl",
      { targetVersion: "3.2.1" },
    ),
    make(
      {
        info: {
          name: "Exact wire methods",
          schema:
            "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
        },
        auth: { type: "noauth" },
        item: exactMethods.map((method) => ({
          name: method,
          request: { method, url: "https://api.example.com/pets" },
        })),
      },
      "methods.postman.json",
      { targetVersion: "3.2.0" },
    ),
    make(
      "type Query { upper: String lower: String customGet: String fixedGet: String }",
      "methods.graphql",
      {
        targetVersion: "3.2.1",
        httpMappings: Object.fromEntries(
          [
            ["upper", "COPY"],
            ["lower", "copy"],
            ["customGet", "get"],
            ["fixedGet", "GET"],
          ].map(([field, method]) => [
            `Query.${field}`,
            {
              method,
              path: "/pets",
              server: "https://api.example.com",
              responseStatus: "200",
              security: [],
            },
          ]),
        ),
      },
    ),
    make("curl -X get https://api.example.com/pets", "older-method.curl", {
      targetVersion: "3.1.2",
    }),
    make("curl -X COP/Y https://api.example.com/pets", "invalid-method.curl", {
      targetVersion: "3.2.1",
    }),
  );
  for (const location of ["request", "response"]) {
    requests.push(
      make(
        {
          asyncapi: "3.0.0",
          info: { title: "Authored HTTP projection", version: "1" },
          channels: {
            events: {
              messages: {
                event: {
                  payload: {
                    schemaFormat:
                      "application/vnd.apache.avro+json;version=1.9.0",
                    schema: { type: "record", name: "Event", fields: [] },
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
        },
        "replacement.json",
        {
          httpMappings: {
            receive: {
              method: "POST",
              path: "/events",
              server: "https://api.example.com",
              responseStatus: "202",
              security: [],
              payloadLocation: location,
              [location + "Schema"]: { type: "string" },
            },
          },
        },
      ),
    );
  }
  requests.push(
    make(
      {
        asyncapi: "3.0.0",
        info: { title: "Mixed payload ownership", version: "1" },
        components: {
          schemas: {
            Unused: {
              dependencies: { id: ["name"] },
              $ref: "https://never-read.invalid/schema",
            },
          },
        },
        channels: {
          source: { messages: { event: { payload: { type: "string" } } } },
          replacement: {
            messages: {
              event: {
                payload: {
                  schemaFormat: "application/vnd.apache.avro+json",
                  schema: { type: "record", name: "Event", fields: [] },
                },
              },
            },
          },
        },
        operations: {
          one: { action: "receive", channel: { $ref: "#/channels/source" } },
          two: {
            action: "receive",
            channel: { $ref: "#/channels/replacement" },
          },
        },
      },
      "mixed-events.json",
      {
        httpMappings: {
          one: {
            method: "POST",
            path: "/one",
            server: "https://api.example.com",
            responseStatus: "202",
            security: [],
            payloadLocation: "request",
          },
          two: {
            method: "POST",
            path: "/two",
            server: "https://api.example.com",
            responseStatus: "202",
            security: [],
            payloadLocation: "request",
            requestSchema: { type: "string" },
          },
        },
      },
    ),
  );
  // The component's owning document controls $ref sibling semantics. Exercise
  // both the legacy blocker and modern constraint preservation in every runtime.
  for (const version of ["3.0.4", "3.1.2", "3.2.1"]) {
    requests.push(
      make(
        {
          asyncapi: "3.0.0",
          info: { title: "Imported schema dialect", version: "1" },
          channels: {
            events: {
              messages: {
                event: {
                  payload: { $ref: "models.json#/components/schemas/Name" },
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
        },
        "events.json",
        {
          httpMappings: {
            receive: {
              method: "POST",
              path: "/events",
              server: "https://api.example.com",
              responseStatus: "202",
              security: [],
              payloadLocation: "request",
            },
          },
        },
        {
          "models.json": JSON.stringify({
            openapi: version,
            info: { title: "Models", version: "1" },
            paths: {},
            components: {
              schemas: {
                Base: { type: "string" },
                Name: { $ref: "#/components/schemas/Base", minLength: 2 },
              },
            },
          }),
        },
      ),
    );
  }
  const oauth = {
    type: "oauth2",
    grantType: "client_credentials",
    accessTokenUrl: "https://auth.example.com/token",
    scope: "{{permissions}}",
  };
  requests.push(
    make(
      {
        _type: "export",
        __export_format: 4,
        resources: [
          { _id: "workspace", _type: "workspace", name: "OAuth scopes" },
          {
            _id: "request",
            _type: "request",
            parentId: "workspace",
            name: "Read",
            method: "GET",
            url: "https://api.example.com/pets",
            authentication: oauth,
          },
        ],
      },
      "insomnia-scopes-v4.json",
      { variables: { permissions: "pets:read pets:write pets:read" } },
    ),
    make(
      {
        type: "collection.insomnia.rest/5.0",
        name: "OAuth scopes",
        collection: [
          {
            name: "Read",
            method: "GET",
            url: "https://api.example.com/pets",
            authentication: oauth,
          },
        ],
      },
      "insomnia-scopes-v5.json",
      { variables: { permissions: "pets:read pets:write pets:read" } },
    ),
    make(
      {
        info: {
          name: "Inherited OAuth scopes",
          schema:
            "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
        },
        auth: {
          type: "oauth2",
          oauth2: [
            { key: "grant_type", value: "client_credentials" },
            { key: "accessTokenUrl", value: "https://auth.example.com/token" },
            { key: "scope", value: "pets:read pets:write" },
          ],
        },
        item: [
          {
            name: "Folder",
            auth: null,
            item: [
              {
                name: "Read",
                request: {
                  method: "GET",
                  url: "https://api.example.com/pets",
                  auth: null,
                },
              },
            ],
          },
        ],
      },
      "postman-null-auth.json",
    ),
  );
  requests.push(
    make(
      {
        $kind: "collection",
        name: "Postman 3 HTTP",
        auth: { type: "noauth" },
        variables: [{ key: "base", value: "https://api.example.com" }],
        items: [
          {
            $kind: "collection",
            name: "Pets",
            items: [
              {
                $kind: "http-request",
                name: "Create",
                method: "POST",
                url: "{{base}}/pets/:id?stale=1",
                pathVariables: [{ key: "id", value: "123" }],
                queryParams: [
                  { key: "tag", value: "猫" },
                  { key: "tag", value: "Café" },
                ],
                body: {
                  type: "json",
                  content: '{"id":18446744073709551615}',
                  $type: '{"type":"object","required":["id"]}',
                },
                examples: [
                  {
                    $kind: "http-example",
                    name: "Created",
                    response: {
                      statusCode: 201,
                      statusText: "Created",
                      headers: [
                        {
                          key: "X-Count",
                          value: "3",
                          $type: "integer",
                          description: "Remaining pets",
                        },
                      ],
                      body: {
                        type: "json",
                        content: '{"id":18446744073709551615}',
                      },
                    },
                  },
                ],
                scripts: [
                  {
                    type: "beforeRequest",
                    code: "throw new Error('never execute')",
                  },
                ],
              },
            ],
          },
        ],
      },
      "postman-v3-http.json",
    ),
    make(
      {
        $kind: "collection",
        name: "Postman 3 auth",
        auth: {
          type: "oauth2",
          id: "read-write",
          name: "API access",
          description: "OAuth credentials inherited by HTTP requests",
          credentials: [
            { key: "grant_type", value: "client_credentials" },
            { key: "accessTokenUrl", value: "https://auth.example.com/token" },
            { key: "scope", value: "pets:read pets:write" },
          ],
        },
        items: [
          {
            $kind: "http-request",
            name: "Read",
            method: "GET",
            url: "https://api.example.com/pets",
            auth: { type: "inherit", credentials: [] },
          },
        ],
      },
      "postman-v3-auth.json",
    ),
    make(
      {
        $kind: "collection",
        name: "Postman 3 conditional auth",
        auth: [
          {
            id: "one",
            name: "Conditional",
            type: "bearer",
            credentials: [],
            rules: [{ key: "url", value: "*.example.com" }],
          },
        ],
        items: [
          {
            $kind: "http-request",
            name: "Read",
            method: "GET",
            url: "https://api.example.com/pets",
          },
        ],
      },
      "postman-v3-rules.json",
    ),
    make(
      {
        $kind: "collection",
        name: "Postman 3 protocol",
        items: [{ $kind: "grpc-request", name: "RPC" }],
      },
      "postman-v3-protocol.json",
    ),
  );
  const inheritedWsdl = readFileSync(
    new URL("../fixtures/wsdl/inheritance.wsdl", import.meta.url),
    "utf8",
  );
  requests.push(
    ...wsdlMethodCases().map(({ request }) => request),
    ...wsdlDefaultCases().map(({ request }) => request),
    make(
      inheritedWsdl,
      "inheritance.wsdl",
      { apiVersion: "1" },
      {
        "base.wsdl": readFileSync(
          new URL("../fixtures/wsdl/inheritance-base.wsdl", import.meta.url),
          "utf8",
        ),
      },
    ),
    make(
      '<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:cycle" targetNamespace="urn:cycle"><interface name="Self" extends="t:Self"/></description>',
      "inheritance-cycle.wsdl",
    ),
  );
  const versions = [
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
  ];
  return [
    ...["loginUser", "logoutUser", "placeOrder", "ProvisionGetToken"].map(
      (operationId) => {
        const document = structuredClone(modern);
        document.paths["/pets"].get.operationId = operationId;
        document.paths["/pets"].get.tags = ["Provision"];
        return { abiVersion: 1, action: "convert", request: make(document) };
      },
    ),
    ...draft7Requests().map((request) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...inheritedDraft7Requests().map((request) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...launchCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...requests.map((request) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...raml08Cases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...ramlTemplateCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...ramlSerializationCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...draft201909Cases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...postmanCredentialCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...postmanAuthCases().map((request) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...postmanSchemaCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...schemaResourceCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...ramlSchemaTextCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...ramlXmlCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...ramlFragmentCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...["Overlay", "Extension"].map((kind) => ({
      abiVersion: 1,
      action: "convert",
      request: make(
        `#%RAML 1.0 ${kind}\nextends: api.raml\ntitle: Localized API\n/pets:\n  get: {}\n`,
        "pasted.txt",
      ),
    })),
    ...importedOpenapi30Cases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...schemaDefinitionCases().map(({ request }) => ({
      abiVersion: 1,
      action: "convert",
      request,
    })),
    ...versions.map((targetVersion) => ({
      abiVersion: 1,
      action: "convert",
      request: make(modern, "source.json", { targetVersion }),
    })),
    {
      abiVersion: 1,
      action: "convert",
      request: make(
        '{"openapi":"3.1.2","info":{"title":"Exact","version":"1"},"paths":{},"components":{"schemas":{"Value":{"type":"integer","example":18446744073709551615}}}}',
      ),
    },
    {
      abiVersion: 1,
      action: "convert",
      request: make(
        '{"openapi":"3.1.2","info":{"title":"Decimal","version":"1"},"paths":{},"components":{"schemas":{"Exact":{"type":"number","example":0.100000000000000005},"Overflow":{"example":1e400},"Fraction":{"example":9007199254740993.25}}}}',
      ),
    },
    { abiVersion: 1, action: "formats" },
    { abiVersion: 1, action: "detect", request: requests[2] },
    { abiVersion: 1, action: "bundle", request: requests[9] },
    { abiVersion: 1, action: "review", request: requests[2] },
    { abiVersion: 1, action: "batch", requests: [requests[0], make("{")] },
    { abiVersion: 1, action: "convert", request: make("{") },
    {
      abiVersion: 1,
      action: "convert",
      request: make(modern, "source.json", { targetVersion: "3.99.0" }),
    },
    { abiVersion: 0, action: "formats" },
    { abiVersion: 1, action: "unknown" },
  ];
}
