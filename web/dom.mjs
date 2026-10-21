/** Customer content is always text or a form value, never HTML. */
export const $ = (id) => document.getElementById(id);
let fieldSequence = 0;
export function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}
export function button(text, handler, className = "secondary") {
  const node = element("button", text, className);
  node.type = "button";
  node.addEventListener("click", handler);
  return node;
}
export function field(
  parent,
  label,
  value = "",
  { options, multiline = false, type = "text" } = {},
) {
  const wrapper = element("div", undefined, "mapping-field");
  const caption = element("label", label);
  const input = element(options ? "select" : multiline ? "textarea" : "input");
  input.id = `field-${++fieldSequence}`;
  caption.htmlFor = input.id;
  if (options)
    for (const value of options) {
      const option = element("option", value);
      option.value = value;
      input.append(option);
    }
  if (!options && !multiline) input.type = type;
  input.value = value;
  input.spellcheck = false;
  wrapper.append(caption, input);
  parent.append(wrapper);
  return input;
}
