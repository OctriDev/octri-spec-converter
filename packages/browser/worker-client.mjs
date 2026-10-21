/** Own a worker, its deadlines and cancellation as one explicit resource. */
import { createClient } from "./client.mjs";

export function createWorkerClient({
  timeoutMs = 30000,
  workerUrl = new URL("./worker.mjs", import.meta.url),
} = {}) {
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 30000)
    throw new RangeError("Worker timeout must be 1..30000 ms");
  let worker;
  let sequence = 0;
  let disposed = false;
  const pending = new Map();
  const stop = (code, message) => {
    worker?.terminate();
    worker = undefined;
    for (const { reject, timer } of pending.values()) {
      clearTimeout(timer);
      reject(Object.assign(new Error(message), { code }));
    }
    pending.clear();
  };
  const start = () => {
    if (worker) return worker;
    worker = new Worker(workerUrl, {
      type: "module",
      name: "Octri Spec Converter",
    });
    worker.onmessage = ({ data }) => {
      const task = pending.get(data?.id);
      if (!task) return;
      pending.delete(data.id);
      clearTimeout(task.timer);
      if (data.error)
        task.reject(
          Object.assign(new Error(data.error.message), {
            code: data.error.code,
          }),
        );
      else if (typeof data.response !== "string")
        task.reject(new Error("Invalid converter worker response"));
      else task.resolve(data.response);
    };
    worker.onerror = () =>
      stop(
        "WORKER_CRASH",
        "Converter worker failed; retry starts a fresh engine",
      );
    worker.onmessageerror = () =>
      stop("WORKER_MESSAGE", "Could not decode worker response");
    return worker;
  };
  const client = createClient((envelope) => {
    if (disposed)
      return Promise.reject(new Error("Converter worker client is disposed"));
    if (pending.size >= 16)
      return Promise.reject(
        Object.assign(new Error("Converter worker queue is full"), {
          code: "WORKER_BUSY",
        }),
      );
    return new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(
        () =>
          stop(
            "WORKER_TIMEOUT",
            "Conversion exceeded its time budget; worker terminated",
          ),
        timeoutMs,
      );
      pending.set(id, { resolve, reject, timer });
      try {
        start().postMessage({ id, envelope });
      } catch (error) {
        pending.delete(id);
        clearTimeout(timer);
        reject(error);
      }
    });
  });
  return Object.freeze({
    ...client,
    cancel: () => stop("CANCELLED", "Conversion cancelled"),
    dispose: () => {
      disposed = true;
      stop("DISPOSED", "Converter worker client disposed");
    },
  });
}
