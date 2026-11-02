// Behavior of the dialog's focus handling under repeated re-rendering (the deployment snapshot is polled every
// few seconds and re-renders the open dialog each time). Real React, real DOM (jsdom), no mocks of the hook.
import assert from "node:assert/strict";
import test from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', { pretendToBeVisual: true });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "Event", "Node"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { useDialogFocus } = await import("../hooks/useDialogFocus.ts");
const h = React.createElement;
const { act, useEffect, useRef } = React;

/** The previous implementation, kept here only to prove these tests can see the bug. */
function useOldDialogFocus(ref, onClose) {
  useEffect(() => {
    ref.current?.querySelector("input, select, button.btn--primary, button")?.focus();
    const onKey = (e) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
}

function makeDialog(useFocus) {
  return function Dialog({ onClose, tick }) {
    const ref = useRef(null);
    useFocus(ref, onClose);
    return h("div", { ref, role: "dialog", "data-tick": tick },
      h("input", { id: "name", defaultValue: "" }),
      h("input", { id: "value", defaultValue: "" }),
      h("select", { id: "scope", defaultValue: "environment" }, h("option", { value: "environment" }, "Environment"), h("option", { value: "global" }, "Global")),
      h("button", { className: "btn btn--primary" }, "Save"));
  };
}

async function mount(useFocus) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const Dialog = makeDialog(useFocus);
  const closes = [];
  let tick = 0;
  // every render hands the dialog a brand-new onClose, as the view does on each poll
  const render = async () => { const id = tick; await act(async () => root.render(h(Dialog, { onClose: () => closes.push(id), tick: tick++ }))); };
  await render();
  return { host, root, closes, render, q: (s) => host.querySelector(s), unmount: () => act(async () => root.unmount()) };
}

const POLLS = 12; // 12 polls = 36 s of the 3 s cycle

test("the dialog focuses its first field when it opens", async () => {
  const d = await mount(useDialogFocus);
  assert.equal(document.activeElement, d.q("#name"));
  await d.unmount();
});

test("repeated re-renders leave focus, the caret and typed text exactly where the person left them", async () => {
  const d = await mount(useDialogFocus);
  const value = d.q("#value");
  value.focus();
  value.value = "ui-test-value";
  value.setSelectionRange(3, 3);
  for (let i = 0; i < POLLS; i++) await d.render();
  assert.equal(document.activeElement, value, "focus stayed in the Value field");
  assert.equal(value.value, "ui-test-value");
  assert.deepEqual([value.selectionStart, value.selectionEnd], [3, 3], "the caret did not move");
  assert.equal(d.q("#name").value, "");
  await d.unmount();
});

test("an open dropdown is never blurred by re-renders", async () => {
  const d = await mount(useDialogFocus);
  const select = d.q("#scope");
  select.focus();
  let blurs = 0;
  select.addEventListener("blur", () => blurs++);
  for (let i = 0; i < POLLS; i++) await d.render();
  assert.equal(blurs, 0, "a blur is what closes a native dropdown");
  assert.equal(document.activeElement, select);
  assert.equal(select.value, "environment");
  await d.unmount();
});

test("the dialog is not remounted by re-rendering: the same DOM nodes survive", async () => {
  const d = await mount(useDialogFocus);
  const before = [d.q("#name"), d.q("#value"), d.q("#scope")];
  for (let i = 0; i < POLLS; i++) await d.render();
  assert.deepEqual([d.q("#name"), d.q("#value"), d.q("#scope")].map((n, i) => n === before[i]), [true, true, true]);
  await d.unmount();
});

test("Escape calls the latest onClose, once, and only while the dialog is open", async () => {
  const d = await mount(useDialogFocus);
  for (let i = 0; i < 3; i++) await d.render(); // renders 0..3; the latest onClose reports 3
  document.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Escape" }));
  assert.deepEqual(d.closes, [3]);
  document.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "a" }));
  assert.deepEqual(d.closes, [3], "other keys do nothing");
  await d.unmount();
  document.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Escape" }));
  assert.deepEqual(d.closes, [3], "no listener is left behind after the dialog closes");
});

test("control: the previous implementation does steal focus and blur the dropdown, so these tests can fail", async () => {
  const d = await mount(useOldDialogFocus);
  const select = d.q("#scope");
  select.focus();
  let blurs = 0;
  select.addEventListener("blur", () => blurs++);
  await d.render();
  assert.equal(document.activeElement, d.q("#name"), "focus was pulled back to the first field");
  assert.ok(blurs >= 1);
  await d.unmount();
});
