/** In-memory source inventory. No storage, resolver or network authority. */
export const DEFAULT_URI = "memory://project/source.yaml";
const FILE_BYTES = 5 * 1024 * 1024;
const TOTAL_BYTES = 20 * 1024 * 1024;
const encoder = new TextEncoder();

export function checkSources(files) {
  if (!files.size || files.size > 100)
    throw new RangeError("Supply between 1 and 100 source files.");
  let total = 0;
  for (const [uri, content] of files) {
    const url = new URL(uri);
    if (url.hash || url.username || url.password)
      throw new TypeError(
        "Source URIs cannot contain fragments or credentials.",
      );
    const bytes = encoder.encode(content).length;
    if (bytes > FILE_BYTES)
      throw new RangeError(`${fileName(uri)} exceeds 5 MiB.`);
    total += bytes;
  }
  if (total > TOTAL_BYTES)
    throw new RangeError(
      "Combined sources exceed 20 MiB. Split the conversion.",
    );
}

export function fileName(uri) {
  try {
    return decodeURIComponent(
      new URL(uri).pathname.split("/").pop() || "source",
    );
  } catch {
    return "source";
  }
}

export async function readText(file, limit = FILE_BYTES) {
  if (file.size > limit)
    throw new RangeError(`${file.name} exceeds the file byte limit.`);
  return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
    await file.arrayBuffer(),
  );
}

export async function uploadedSources(list, existing) {
  if (list.length > 100) throw new RangeError("Choose at most 100 files.");
  const next = new Map(existing);
  for (const file of list) {
    const path = file.webkitRelativePath || file.name;
    const segments = path.split("/");
    if (
      segments.some(
        (segment) => !segment || segment === "." || segment === "..",
      )
    )
      throw new TypeError("File paths must contain ordinary relative names.");
    const uri =
      "memory://project/" + segments.map(encodeURIComponent).join("/");
    if (next.has(uri))
      throw new TypeError(
        `${path} already exists. Remove it before adding a replacement.`,
      );
    next.set(uri, await readText(file));
    checkSources(next);
  }
  return next;
}

function downloadDocument() {
  try {
    // Same-origin hosts own the visible browsing surface and its download
    // manager. Keep cross-origin embeds in their own document and policy scope.
    if (
      window.parent !== window &&
      window.parent.location.origin === window.location.origin
    )
      return window.parent.document;
  } catch {
    // Reading a cross-origin parent's location is forbidden by the browser.
  }
  return document;
}

export function download(name, content, type = "application/json") {
  const host = downloadDocument();
  const url = URL.createObjectURL(new Blob([content], { type }));
  const anchor = host.createElement("a");
  anchor.href = url;
  anchor.download = name.replace(/[^a-zA-Z0-9._-]/g, "_");
  host.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function selection(text, location) {
  if (!location.line) {
    if (!Number.isInteger(location.offset)) return 0;
    return new TextDecoder("utf-8", { ignoreBOM: true }).decode(
      encoder.encode(text).slice(0, location.offset),
    ).length;
  }
  const lines = text.split("\n");
  return (
    lines
      .slice(0, location.line - 1)
      .reduce((length, line) => length + line.length + 1, 0) +
    Array.from(lines[location.line - 1] || "")
      .slice(0, Math.max(0, (location.column || 1) - 1))
      .join("").length
  );
}

export async function fetchSource(value, signal, endpoint) {
  const url = new URL(value.trim());
  if (
    !["https:", "http:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.hash
  )
    throw new TypeError(
      "Use an HTTP or HTTPS source URL without credentials or a fragment.",
    );
  const response = endpoint
    ? await fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ url: url.href }),
        credentials: "omit",
        cache: "no-store",
        signal,
      })
    : await fetch(url, { credentials: "omit", cache: "no-store", signal });
  if (!response.ok)
    throw new Error(
      `Source server returned HTTP ${response.status}. Download the source directly and add it as a file if access restrictions apply.`,
    );
  if (!response.body) throw new Error("Source response has no body.");
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  let bytes = 0;
  const chunks = [];
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > FILE_BYTES)
        throw new RangeError("Fetched source exceeds 5 MiB.");
      chunks.push(decoder.decode(value, { stream: true }));
    }
    chunks.push(decoder.decode());
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  const final = new URL(
    response.headers.get("x-octri-source-url") ||
      (endpoint ? url.href : response.url),
  );
  if (
    !["https:", "http:"].includes(final.protocol) ||
    final.username ||
    final.password ||
    final.hash
  )
    throw new Error("Source adapter returned an invalid retrieval URI.");
  return { uri: final.href, content: chunks.join("") };
}
