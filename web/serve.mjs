/** Local static preview. Serves only the browser product and its engine assets. */
import { createServer } from "node:http";
import { readFile, realpath, stat } from "node:fs/promises";
import { resolve, extname, sep } from "node:path";
import { fileURLToPath } from "node:url";

export async function serve({ port = 0 } = {}) {
  const root = await realpath(fileURLToPath(new URL("..", import.meta.url)));
  const types = {
    ".html": "text/html; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".json": "application/json",
    ".wasm": "application/wasm",
    ".svg": "image/svg+xml",
  };
  const server = createServer(async (request, response) => {
    response.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self' https: http:; object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'none'",
    );
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Referrer-Policy", "no-referrer");
    response.setHeader("Cache-Control", "no-store");
    try {
      const path = decodeURIComponent(
        new URL(request.url, "http://localhost").pathname,
      );
      if (path === "/") {
        response.writeHead(302, { Location: "/web/index.html" }).end();
        return;
      }
      if (
        !["GET", "HEAD"].includes(request.method) ||
        !(path.startsWith("/web/") || path.startsWith("/packages/browser/"))
      ) {
        response.writeHead(404).end();
        return;
      }
      const absolute = await realpath(resolve(root, "." + path));
      if (
        !absolute.startsWith(root + sep) ||
        !(await stat(absolute)).isFile() ||
        !types[extname(absolute)]
      ) {
        response.writeHead(404).end();
        return;
      }
      response.setHeader("Content-Type", types[extname(absolute)]);
      if (request.method === "HEAD") response.end();
      else response.end(await readFile(absolute));
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const { origin } = await serve({ port: Number(process.env.PORT || "4178") });
  console.log(`Spec Converter preview: ${origin}/web/index.html`);
}
