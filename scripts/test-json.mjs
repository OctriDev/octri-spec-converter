import assert from "node:assert/strict";
import test from "node:test";
import {
  LosslessNumber,
  parseJson,
  stringifyJson,
  object,
  createClient,
} from "../packages/shared/client.mjs";

test("exact decimal and exponent tokens survive report transport", () => {
  for (const token of [
    "0.100000000000000005",
    "9007199254740993.25",
    "1e400",
    "1e-400",
    "1e999999999999999999999",
  ]) {
    const decoded = parseJson(`{"example":${token}}`);
    assert.ok(decoded.example instanceof LosslessNumber);
    assert.equal(stringifyJson(decoded), `{"example":${token}}`);
    assert.throws(() => +decoded.example, /explicitly/);
  }
  assert.equal(parseJson("18446744073709551616"), 18446744073709551616n);
  for (const token of ["0", "-0", "0.1", "1.00e0", "12.5", "1e-20"])
    assert.equal(typeof parseJson(token), "number");
});

test("codec works without JSON.rawJSON or reviver source support", () => {
  const original = JSON.rawJSON;
  try {
    JSON.rawJSON = undefined;
    const source =
      '{"exact":0.100000000000000005,"integer":18446744073709551616}';
    assert.equal(stringifyJson(parseJson(source)), source);
  } finally {
    JSON.rawJSON = original;
  }
});

test("strings, escapes and prototype-shaped keys remain inert", () => {
  const source =
    '{"__proto__":{"safe":1},"quote":"\\\"12.5\\\"","\\u006b":"1e400"}';
  const decoded = parseJson(source);
  assert.equal(Object.getPrototypeOf(decoded), Object.prototype);
  assert.equal(Object.prototype.safe, undefined);
  assert.deepEqual(JSON.parse(stringifyJson(decoded)), JSON.parse(source));
  assert.throws(() => parseJson('{"k":1,"\\u006b":2}'), /Duplicate/);
});

test("invalid grammar and nesting cannot bypass bounded decoder", () => {
  for (const source of [
    "01",
    "+1",
    "1.",
    ".1",
    "NaN",
    "1e",
    "[1,]",
    '{"a":1,}',
    '"unterminated',
    '"\\q"',
    "true false",
  ])
    assert.throws(() => parseJson(source), SyntaxError, source);
  assert.throws(
    () => parseJson("[".repeat(258) + "0" + "]".repeat(258)),
    /nesting/,
  );
  assert.throws(() => new LosslessNumber("1,2"), /number token/);
});

test("object and envelope encoding reject hooks without invoking them", async () => {
  let invoked = 0;
  const hooked = {
    get value() {
      invoked++;
      return 1;
    },
  };
  assert.throws(() => object(hooked), /accessors/);
  const client = createClient(() => {
    invoked++;
    return '{"Ok":null}';
  });
  await assert.rejects(client.call(hooked), /accessors/);
  assert.equal(invoked, 0);
  assert.throws(
    () => stringifyJson({ value: Number.MAX_SAFE_INTEGER + 1 }),
    /BigInt/,
  );
  assert.throws(() => stringifyJson({ value: Infinity }), /JSON values/);
  const cycle = {};
  cycle.self = cycle;
  assert.throws(() => stringifyJson(cycle), /Circular/);
  assert.throws(() => stringifyJson([, 1]), /Sparse/);
});

test("formatted exact output remains JSON", () => {
  const source = {
    decimal: new LosslessNumber("0.100000000000000005"),
    array: [42n, "text"],
  };
  const formatted = stringifyJson(source, 2);
  assert.equal(
    stringifyJson(parseJson(formatted)),
    '{"decimal":0.100000000000000005,"array":[42,"text"]}',
  );
});
