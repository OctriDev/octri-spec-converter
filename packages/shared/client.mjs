/** Transport facade shared by native Node and browser WASM packages. */
import { encodeInput } from "./json.mjs";

export { parseJson, LosslessNumber } from "./numbers.mjs";
import { parseJson } from "./numbers.mjs";

export function stringifyJson(value, space) {
  return encodeInput(
    value,
    { maxDocumentBytes: 128 * 1024 * 1024, maxDepth: 256, maxNodes: 4_000_000 },
    space,
  );
}

export class ConversionError extends Error {
  constructor(error) {
    super(error.message);
    this.name = "ConversionError";
    this.code = error.code;
    this.diagnostics = error.diagnostics ?? [];
  }
}

export function createClient(execute) {
  const executeJson = async (input) => {
    if (typeof input !== "string")
      throw new TypeError("Raw envelope must be JSON text");
    if (new TextEncoder().encode(input).length > 128 * 1024 * 1024)
      throw new RangeError("Converter envelope exceeds 128 MiB");
    return execute(input);
  };
  const call = async (envelope) => {
    const response = parseJson(await executeJson(stringifyJson(envelope)));
    if (response.Err) throw new ConversionError(response.Err);
    if (!("Ok" in response))
      throw new Error("Invalid converter response envelope");
    return response.Ok;
  };
  return Object.freeze({
    executeJson,
    call,
    convert: (request) => call({ abiVersion: 1, action: "convert", request }),
    review: (request) => call({ abiVersion: 1, action: "review", request }),
    detect: (request) => call({ abiVersion: 1, action: "detect", request }),
    batch: (requests) => call({ abiVersion: 1, action: "batch", requests }),
    bundle: (request) => call({ abiVersion: 1, action: "bundle", request }),
    compare: (before, after) =>
      call({ abiVersion: 1, action: "compare", before, after }),
    export: (report, format = "json") =>
      call({ abiVersion: 1, action: "export", report, format }),
    formats: () => call({ abiVersion: 1, action: "formats" }),
  });
}

export function text(content, uri = "memory://convert/source.yaml") {
  if (typeof content !== "string")
    throw new TypeError("Source content must be a string");
  return { uri, content, generated: false };
}

export function object(document, uri = "memory://convert/source.json") {
  if (
    document === null ||
    typeof document !== "object" ||
    Array.isArray(document)
  )
    throw new TypeError("Source must be a JSON object");
  return {
    uri,
    content: encodeInput(document, { maxDocumentBytes: 5 * 1024 * 1024 }),
    generated: true,
  };
}
