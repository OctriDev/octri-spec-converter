/** Ordered credential sets shared by native, Node and browser consumers. */
import { createHash } from "node:crypto";

export function postmanAuthCases() {
  const basic = {
    id: "protected",
    name: "Protected",
    type: "basic",
    credentials: [],
  };
  const publicSet = { id: "public", name: "Public", type: "noauth" };
  return [
    [publicSet, basic],
    [{ ...basic, rules: [] }, publicSet],
    [
      {
        ...basic,
        rules: [{ key: "url", value: "https://other.example.test/*" }],
      },
      publicSet,
    ],
    [{ ...publicSet, disabled: true }, basic],
  ].map((auth) => ({
    input: {
      uri: "memory://credentials/collection.json",
      generated: false,
      content: JSON.stringify({
        $kind: "collection",
        name: "Ordered credentials",
        auth,
        items: [
          {
            $kind: "http-request",
            name: "List items",
            method: "GET",
            url: "https://api.example.test/items",
          },
        ],
      }),
    },
    config: { targetVersion: "3.2.1" },
  }));
}

/** Explicit security choices preserve conditional source rules as evidence. */
export function postmanCredentialCases() {
  const pending = postmanAuthCases()[2];
  const source = JSON.parse(pending.input.content);
  source.auth[0].type = "bearer";
  source.auth[0].credentials = [
    { key: "token", value: "credential-secret-never-in-output" },
  ];
  pending.input.content = JSON.stringify(source);
  const selection = {
    sourceUri: pending.input.uri,
    sourceSha256: createHash("sha256")
      .update(pending.input.content)
      .digest("hex"),
    authPointer: "/auth",
    credentialIndex: 0,
  };
  const selected = structuredClone(pending);
  selected.config.credentialSelections = { "/items/0": selection };
  const stale = structuredClone(selected);
  stale.input.content += " ";
  const unused = structuredClone(selected);
  unused.config.credentialSelections = { "/items/99": selection };
  const wideIndex = structuredClone(selected);
  wideIndex.config.credentialSelections["/items/0"].credentialIndex =
    4294967296;
  const maxIndex = structuredClone(selected);
  maxIndex.config.credentialSelections["/items/0"].credentialIndex =
    18446744073709551615n;
  return [
    { kind: "pending", request: pending },
    { kind: "selected", request: selected },
    { kind: "stale", request: stale },
    { kind: "unused", request: unused },
    { kind: "wide-index", request: wideIndex },
    { kind: "max-index", request: maxIndex },
  ];
}
