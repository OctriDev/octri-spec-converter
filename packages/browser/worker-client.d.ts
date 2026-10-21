import type { ConverterClient } from "./index.js";
export interface WorkerConverterClient extends ConverterClient {
  cancel(): void;
  dispose(): void;
}
export function createWorkerClient(options?: {
  timeoutMs?: number;
  workerUrl?: URL;
}): WorkerConverterClient;
