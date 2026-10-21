import type * as API from "./index.d.ts" with { "resolution-mode": "import" };
declare const bindings: typeof API.Converter & {
  Converter: typeof API.Converter;
  ConversionError: typeof API.ConversionError;
  LosslessNumber: typeof API.LosslessNumber;
  text: typeof API.text;
  object: typeof API.object;
  parseJson: typeof API.parseJson;
  stringifyJson: typeof API.stringifyJson;
  default: typeof API.Converter;
};
export = bindings;
