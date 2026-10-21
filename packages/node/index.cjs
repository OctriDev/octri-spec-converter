const native = require("./load-native.cjs")();
const {
  createClient,
  ConversionError,
  LosslessNumber,
  text,
  object,
  parseJson,
  stringifyJson,
} = require("./client.mjs");
const Converter = createClient(native.execute);
module.exports = {
  ...Converter,
  Converter,
  ConversionError,
  LosslessNumber,
  text,
  object,
  parseJson,
  stringifyJson,
  default: Converter,
};
