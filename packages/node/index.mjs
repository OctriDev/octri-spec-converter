import { createRequire } from "node:module";
import { createClient } from "./client.mjs";
export {
  ConversionError,
  LosslessNumber,
  text,
  object,
  parseJson,
  stringifyJson,
} from "./client.mjs";
const require = createRequire(import.meta.url);
const native = require("./load-native.cjs")();
export const Converter = createClient(native.execute);
export default Converter;
