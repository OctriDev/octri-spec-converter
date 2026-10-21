/** Dedicated offline engine worker. Source text never becomes a URL or script. */
import { initialize, Converter } from "./index.mjs";
self.onmessage = async ({ data }) => {
  if (!data || typeof data.id !== "number" || typeof data.envelope !== "string")
    return;
  try {
    await initialize();
    self.postMessage({
      id: data.id,
      response: await Converter.executeJson(data.envelope),
    });
  } catch (error) {
    self.postMessage({
      id: data.id,
      error: {
        code: "WORKER_ENGINE",
        message: error instanceof Error ? error.message : String(error),
      },
    });
  }
};
