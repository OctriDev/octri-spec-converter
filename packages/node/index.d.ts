export class LosslessNumber {
  readonly token: string;
  constructor(token: string);
  toString(): string;
  valueOf(): never;
}
export type JsonValue =
  | null
  | boolean
  | string
  | number
  | bigint
  | LosslessNumber
  | JsonValue[]
  | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };
export type InputFormat =
  | "auto"
  | "openapi"
  | "swagger"
  | "postman"
  | "raml"
  | "insomnia"
  | "har"
  | "curl"
  | "asyncapi"
  | "graphql"
  | "protobuf"
  | "wsdl";
export type TargetVersion =
  | "2.0"
  | "3.0.0"
  | "3.0.1"
  | "3.0.2"
  | "3.0.3"
  | "3.0.4"
  | "3.1.0"
  | "3.1.1"
  | "3.1.2"
  | "3.2.0"
  | "3.2.1";
export type OutputFormat = "json" | "yaml";
export interface Input {
  uri: string;
  content: string;
  generated?: boolean;
}
export interface Limits {
  maxDocumentBytes?: number;
  maxTotalBytes?: number;
  maxDocuments?: number;
  maxNodes?: number;
  maxDepth?: number;
  maxAliases?: number;
  maxOutputBytes?: number;
  maxDecisions?: number;
}
export interface HttpMapping {
  method: string;
  path: string;
  server: string;
  responseStatus: string;
  requestSchema?: JsonValue;
  responseSchema?: JsonValue;
  security?: JsonValue;
  parameterBindings?: Record<
    string,
    { location: "path" | "query" | "header" | "cookie"; name: string }
  >;
  payloadLocation?: "request" | "response";
  requestStream?: StreamBinding;
  responseStream?: StreamBinding;
}
export interface StreamBinding {
  framing: "ndjson" | "json-sequence" | "sse";
  lifecycle: string;
}
export interface MappingRequest {
  key: string;
  source: Location;
  inputFields: string[];
  hasInput: boolean;
  hasOutput: boolean;
  requestStreamRequired: boolean;
  responseStreamRequired: boolean;
  payloadLocationRequired: boolean;
}
export interface CredentialSelection {
  sourceUri: string;
  sourceSha256: string;
  authPointer: string;
  credentialIndex: number;
}
export interface CredentialSelectionRequest {
  name: string;
  request: Location;
  authSource: Location;
  sourceSha256: string;
  choices: {
    index: number;
    id: string | null;
    name: string | null;
    authType: string;
  }[];
  selected: number | null;
}
export interface Config {
  inputFormat?: InputFormat;
  targetVersion?: TargetVersion;
  outputFormat?: OutputFormat;
  bundle?: boolean;
  title?: string;
  apiVersion?: string;
  baseUrl?: string;
  environmentUri?: string;
  environmentId?: string;
  variables?: Record<string, string>;
  httpMappings?: Record<string, HttpMapping>;
  credentialSelections?: Record<string, CredentialSelection>;
  securitySchemes?: Record<string, JsonValue>;
  duplicateChoices?: Record<string, string>;
  overrides?: Record<string, JsonValue>;
  acceptedDecisions?: string[];
  dropUnsupported?: string[];
  limits?: Limits;
}
export interface ConversionRequest {
  input: Input;
  files?: Record<string, string>;
  config?: Config;
}
export interface Location {
  uri: string;
  pointer: string;
  line?: number;
  column?: number;
  offset?: number;
}
export interface Diagnostic {
  code: string;
  severity: "error" | "warning" | "info";
  message: string;
  location: Location;
  related?: Location[];
}
export interface Decision {
  id: string;
  code: string;
  classification:
    | "preserved"
    | "transformed"
    | "inferred"
    | "omitted"
    | "blocked";
  source: Location;
  outputPointers: string[];
  message: string;
  remediation: string | null;
  reviewRequired: boolean;
  reviewed: boolean;
}
export interface DuplicateRequest {
  identity: string;
  candidates: { pointer: string; source: Location }[];
  selected: string | null;
}
export interface Manifest {
  engineVersion: string;
  parserVersion: string;
  processingMode: string;
  root: string;
  sourceFormat: InputFormat;
  targetVersion: TargetVersion;
  files: { uri: string; sha256: string; bytes: number }[];
  config: Config;
}
export interface ConversionReport {
  schemaVersion: string;
  manifest: Manifest;
  status: "complete" | "needs-review" | "blocked";
  valid: boolean;
  handoffAllowed: boolean;
  output: {
    uri: string;
    content: string;
    sha256: string;
    encoding: OutputFormat;
  } | null;
  document: JsonValue | null;
  /** Parser inventory of the emitted document, retained without rerunning conversion. */
  inspection: JsonObject | null;
  decisions: Decision[];
  mappingRequests: MappingRequest[];
  credentialSelectionRequests?: CredentialSelectionRequest[];
  duplicateRequests: DuplicateRequest[];
  diagnostics: Diagnostic[];
  checks: { id: string; status: string; detail: string }[];
  /** Full portable Spec Auditor report. Its own schemaVersion identifies that contract. */
  audit: JsonObject | null;
  original: ConversionRequest;
}
export interface ErrorDetail {
  code: string;
  message: string;
  diagnostics: Diagnostic[];
}
export class ConversionError extends Error {
  code: string;
  diagnostics: Diagnostic[];
  constructor(detail: ErrorDetail);
}
export type Result<T> = { Ok: T } | { Err: ErrorDetail };
export interface FormatCatalogue {
  schemaVersion: string;
  engineVersion: string;
  targets: TargetVersion[];
  inputs: Exclude<InputFormat, "auto">[];
  offline: boolean;
  httpMappingsRequiredForNonHttp: boolean;
}
export interface ConverterClient {
  executeJson(input: string): Promise<string>;
  call(envelope: JsonObject): Promise<JsonValue>;
  convert(request: ConversionRequest): Promise<ConversionReport>;
  review(request: ConversionRequest): Promise<ConversionReport>;
  detect(request: ConversionRequest): Promise<Exclude<InputFormat, "auto">>;
  batch(requests: ConversionRequest[]): Promise<Result<ConversionReport>[]>;
  bundle(request: ConversionRequest): Promise<ConversionReport>;
  compare(
    before: ConversionReport,
    after: ConversionReport,
  ): Promise<JsonObject>;
  export(
    report: ConversionReport,
    format?: "json" | "html" | "manifest" | "artifact",
  ): Promise<JsonValue>;
  formats(): Promise<FormatCatalogue>;
}
export const Converter: ConverterClient;
export default Converter;
export function text(content: string, uri?: string): Input;
export function object(document: JsonObject, uri?: string): Input;
export function parseJson(text: string): JsonValue;
export function stringifyJson(value: unknown, space?: string | number): string;
