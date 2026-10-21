import { numberToken } from "./numbers.mjs";

function bindingError(code, message) {
  return Object.assign(new TypeError(message), { code });
}
/** Validate caller-owned objects before JSON transport. Traversal limits apply
 * to occurrences, so shared-subtree expansion cannot bypass the byte/node caps. */
// Ordinary ASCII needs neither escaping nor UTF-8 expansion.
const needsStringCounting = /["\\\u0000-\u001f\u0080-\uffff]/;
export function encodeInput(value, limits = {}, space) {
  const checkedPrototypes = new Set();
  rejectInheritedHook({}, checkedPrototypes);
  const maxBytes = limits.maxDocumentBytes ?? 16 * 1024 * 1024;
  const maxNodes = limits.maxNodes ?? 1_000_000;
  const maxDepth = Math.min(limits.maxDepth ?? 128, 256);
  const active = new Set();
  let nodes = 0;
  let bytes = 0;
  function charge(count) {
    bytes += count;
    if (bytes > maxBytes)
      throw bindingError(
        "LIMIT_DOCUMENT_BYTES",
        "Encoded object input exceeds byte limit",
      );
  }
  function string(text) {
    const remaining = maxBytes - bytes;
    // Even unescaped ASCII cannot fit; reject before scanning a huge string.
    if (text.length + 2 > remaining) charge(text.length + 2);
    if (!needsStringCounting.test(text)) {
      charge(text.length + 2);
      return;
    }
    // Count JSON escaping and UTF-8 bytes without allocating an encoded copy.
    let size = 2;
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      if (
        code === 34 ||
        code === 92 ||
        code === 8 ||
        code === 9 ||
        code === 10 ||
        code === 12 ||
        code === 13
      )
        size += 2;
      else if (code < 32) size += 6;
      else if (code < 128) size++;
      else if (code < 2048) size += 2;
      else if (
        code >= 0xd800 &&
        code <= 0xdbff &&
        i + 1 < text.length &&
        text.charCodeAt(i + 1) >= 0xdc00 &&
        text.charCodeAt(i + 1) <= 0xdfff
      ) {
        size += 4;
        i++;
      } else if (code >= 0xd800 && code <= 0xdfff) size += 6;
      else size += 3;
      if (size > remaining) {
        charge(size);
      }
    }
    charge(size);
  }
  function check(value, depth) {
    if (++nodes > maxNodes)
      throw bindingError(
        "LIMIT_NODES",
        "Object input exceeds occurrence limit",
      );
    if (depth > maxDepth)
      throw bindingError("LIMIT_DEPTH", "Object input exceeds nesting limit");
    if (value === null) {
      charge(4);
      return;
    }
    if (typeof value === "string") {
      string(value);
      return;
    }
    if (typeof value === "boolean") {
      charge(value ? 4 : 5);
      return;
    }
    if (typeof value === "number" && Number.isFinite(value)) {
      if (Number.isInteger(value) && !Number.isSafeInteger(value))
        throw bindingError(
          "UNSAFE_INTEGER",
          "Use BigInt or original JSON text for integers outside JavaScript's safe range",
        );
      charge(JSON.stringify(value).length);
      return;
    }
    if (typeof value === "bigint") {
      charge(value.toString().length);
      return;
    }
    const exact = numberToken(value);
    if (exact !== undefined) {
      charge(exact.length);
      return;
    }
    if (typeof value !== "object")
      throw bindingError(
        "NON_JSON_VALUE",
        "Object input must contain only JSON values",
      );
    if (active.has(value))
      throw bindingError(
        "CIRCULAR_INPUT",
        "Circular object input is not a JSON document",
      );
    const array = Array.isArray(value);
    if (!array && !plainObject(value))
      throw bindingError(
        "NON_JSON_VALUE",
        "Object input must contain plain objects",
      );
    rejectInheritedHook(value, checkedPrototypes);
    active.add(value);
    charge(2);
    let fields = 0;
    for (const key of Reflect.ownKeys(value)) {
      if (array && key === "length") continue;
      if (typeof key !== "string")
        throw bindingError(
          "NON_JSON_VALUE",
          "Symbol keys cannot be represented as JSON",
        );
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (
        descriptor?.get ||
        descriptor?.set ||
        (key === "toJSON" && typeof descriptor?.value === "function")
      )
        throw bindingError(
          "NON_JSON_VALUE",
          "Object input cannot contain accessors or toJSON hooks",
        );
      if (!descriptor?.enumerable) continue;
      if (array) {
        if (key !== String(fields))
          throw bindingError(
            "NON_JSON_VALUE",
            "Sparse arrays and custom array properties are not JSON values",
          );
      } else {
        string(key);
        charge(1);
      }
      if (fields++) charge(1);
      check(descriptor.value, depth + 1);
    }
    if (array && fields !== value.length)
      throw bindingError("NON_JSON_VALUE", "Sparse arrays are not JSON values");
    active.delete(value);
  }
  check(value, 0);
  // Never call JSON.stringify on containers: it invokes caller-owned toJSON
  // hooks and cannot encode exact number tokens on older browser runtimes.
  const indent =
    typeof space === "number"
      ? " ".repeat(Math.min(10, Math.max(0, Math.trunc(space) || 0)))
      : typeof space === "string"
        ? space.slice(0, 10)
        : "";
  const chunks = [];
  let formattedBytes = 0;
  function append(text) {
    formattedBytes += new TextEncoder().encode(text).length;
    if (formattedBytes > maxBytes)
      throw bindingError(
        "LIMIT_DOCUMENT_BYTES",
        "Encoded JSON exceeds byte limit",
      );
    chunks.push(text);
  }
  function write(item, depth) {
    const exact = numberToken(item);
    if (exact !== undefined) {
      append(exact);
      return;
    }
    if (typeof item === "bigint") {
      append(item.toString());
      return;
    }
    if (item === null || typeof item !== "object") {
      append(JSON.stringify(item));
      return;
    }
    const array = Array.isArray(item);
    const entries = Object.entries(
      Object.getOwnPropertyDescriptors(item),
    ).filter(
      ([key, descriptor]) =>
        descriptor.enumerable && !(array && key === "length"),
    );
    append(array ? "[" : "{");
    entries.forEach(([key, descriptor], index) => {
      if (index) append(",");
      if (indent) append("\n" + indent.repeat(depth + 1));
      if (!array) append(JSON.stringify(key) + (indent ? ": " : ":"));
      write(descriptor.value, depth + 1);
    });
    if (entries.length && indent) append("\n" + indent.repeat(depth));
    append(array ? "]" : "}");
  }
  write(value, 0);
  return chunks.join("");
}
function rejectInheritedHook(value, checked) {
  // JSON.stringify performs a prototype lookup for toJSON, even when the input
  // has no own hook. Inspect descriptors without invoking inherited accessors.
  for (
    let prototype = Object.getPrototypeOf(value);
    prototype !== null;
    prototype = Object.getPrototypeOf(prototype)
  ) {
    if (checked.has(prototype)) return;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, "toJSON");
    if (
      descriptor?.get ||
      descriptor?.set ||
      typeof descriptor?.value === "function"
    )
      throw bindingError(
        "NON_JSON_VALUE",
        "Object input cannot inherit toJSON hooks",
      );
    checked.add(prototype);
  }
}
function plainObject(value) {
  const prototype = Object.getPrototypeOf(value);
  if (prototype === null || prototype === Object.prototype) return true;
  // Plain objects from another realm have their own Object.prototype.
  const constructor = Object.getOwnPropertyDescriptor(prototype, "constructor");
  return (
    Object.getPrototypeOf(prototype) === null &&
    typeof constructor?.value === "function" &&
    Function.prototype.toString.call(constructor.value) ===
      Function.prototype.toString.call(Object)
  );
}
