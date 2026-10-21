/** Exact JSON number tokens that cannot survive JavaScript's Number model. */
const tokens = new WeakMap();
const grammar = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;

export class LosslessNumber {
  constructor(token) {
    if (typeof token !== "string" || !grammar.test(token))
      throw new TypeError("Expected a JSON number token");
    this.token = token;
    tokens.set(this, token);
    Object.freeze(this);
  }
  toString() {
    return tokens.get(this);
  }
  valueOf() {
    throw new TypeError("Convert an exact number explicitly before arithmetic");
  }
}

export function numberToken(value) {
  return tokens.get(value);
}

// Compare decimal values without constructing powers of ten. An exponent beyond
// 15 digits cannot be cancelled by a bounded transport's coefficient length.
function decimal(token) {
  const [mantissa, exponent = "0"] = token.toLowerCase().split("e");
  const negative = mantissa.startsWith("-");
  const unsigned = negative ? mantissa.slice(1) : mantissa;
  const dot = unsigned.indexOf(".");
  let digits = unsigned.replace(".", "").replace(/^0+/, "");
  if (!digits) return "0";
  const scale = exponent.replace(/^[+-]?0*/, "");
  if (scale.length > 15) return null;
  let power = Number(exponent) - (dot < 0 ? 0 : unsigned.length - dot - 1);
  const shortened = digits.replace(/0+$/, "");
  power += digits.length - shortened.length;
  return `${negative ? "-" : ""}${shortened}e${power}`;
}

/** Mathematical identity for transport qualification, without float conversion. */
export function numberIdentity(token) {
  return decimal(token) ?? token.toLowerCase();
}

export function decodeNumber(token) {
  const number = Number(token);
  if (Number.isFinite(number) && decimal(token) === decimal(String(number))) {
    if (!Number.isInteger(number) || Number.isSafeInteger(number))
      return number;
  }
  if (/^-?[0-9]+$/.test(token) && token.length <= 4096) return BigInt(token);
  return new LosslessNumber(token);
}

/** Bounded transport decoder. Rust remains responsible for source semantics. */
export function parseJson(text) {
  if (typeof text !== "string") throw new TypeError("Expected JSON text");
  if (
    text.length > 128 * 1024 * 1024 ||
    new TextEncoder().encode(text).length > 128 * 1024 * 1024
  )
    throw new RangeError("JSON transport exceeds 128 MiB");
  let at = 0;
  let nodes = 0;
  const fail = () => {
    throw new SyntaxError(`Invalid JSON at offset ${at}`);
  };
  function whitespace() {
    while (at < text.length && /[\t\n\r ]/.test(text[at])) at++;
  }
  function string() {
    const start = at++;
    while (at < text.length) {
      const char = text[at++];
      if (char === '"') return JSON.parse(text.slice(start, at));
      if (char === "\\") at++;
    }
    return fail();
  }
  function value(depth) {
    if (++nodes > 4_000_000)
      throw new RangeError("JSON transport exceeds token limit");
    if (depth > 256)
      throw new RangeError("JSON transport exceeds nesting limit");
    whitespace();
    const char = text[at];
    if (char === '"') return string();
    if (char === "{" || char === "[") {
      const object = char === "{";
      const result = object ? {} : [];
      const end = object ? "}" : "]";
      at++;
      whitespace();
      if (text[at] !== end) {
        while (true) {
          let key;
          if (object) {
            if (++nodes > 4_000_000)
              throw new RangeError("JSON transport exceeds token limit");
            if (text[at] !== '"') return fail();
            key = string();
            if (Object.hasOwn(result, key))
              throw new SyntaxError("Duplicate JSON key");
            whitespace();
            if (text[at++] !== ":") return fail();
          }
          const item = value(depth + 1);
          if (object)
            Object.defineProperty(result, key, {
              value: item,
              writable: true,
              enumerable: true,
              configurable: true,
            });
          else result.push(item);
          whitespace();
          if (text[at] !== ",") break;
          at++;
          whitespace();
        }
      }
      if (text[at++] !== end) return fail();
      return result;
    }
    for (const [literal, result] of [
      ["true", true],
      ["false", false],
      ["null", null],
    ]) {
      if (text.startsWith(literal, at)) {
        at += literal.length;
        return result;
      }
    }
    const start = at;
    while (at < text.length && /[0-9eE+.-]/.test(text[at])) at++;
    const token = text.slice(start, at);
    if (!grammar.test(token)) return fail();
    return decodeNumber(token);
  }
  const result = value(0);
  whitespace();
  if (at !== text.length) return fail();
  return result;
}
