// Real-browser layout check for the release card: the stylesheet is the shipped deploy.css, the markup mirrors the card in
// Screens.tsx (class names and order are asserted from the source below), and Chrome measures it at several card widths.
// Needs playwright-core and a Chrome; without them the test FAILS rather than skipping (set HARPOON_NO_BROWSER_TESTS=1 to opt out).
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";

const css = readFileSync(new URL("../views/deploy/deploy.css", import.meta.url), "utf8");
const screens = readFileSync(new URL("../views/deploy/Screens.tsx", import.meta.url), "utf8");
const LONG_ID = "dep_01M7D1AJHSVQ45GXQFTG64ZG5Q";
const LONG_URL = "http://tvzg5ry7dd.harpoon.localhost:7480";
const LONG_CONTAINER = "hp-dep_01M7D1AJHSVQ45GXQFTG64ZG5Q-restored";

const card = (thumb) => `<div class="dp-root"><div class="dp-panel dp-herocard"><div class="dp-panel__body dp-hero" ${thumb ? "data-thumb" : ""}>
  ${thumb ? `<div class="dp-thumb"><div class="dp-thumb__img dp-thumb__none"><b>Preview unavailable</b></div><button class="dp-thumb__refresh" aria-label="Refresh preview">↻</button></div>` : ""}
  <div class="dp-hero__state"><div>
    <div class="dp-row dp-row--wrap"><span class="dp-pill dp-pill--ok">Running</span><b>release rel_${LONG_ID}</b><span class="mono dp-muted">a68140e</span></div>
    <div class="dp-muted">production · container <span class="mono">${LONG_CONTAINER}</span></div>
    <div class="dp-address"><span class="dp-address__label">Application URL</span><span class="dp-address__url mono">${LONG_URL}</span><button class="btn btn--secondary btn--sm">Copy</button><button class="btn btn--secondary btn--sm">Open</button></div>
  </div></div>
  <div class="dp-actions dp-actions--end"><button class="btn btn--primary">Deploy</button><button class="btn btn--secondary">Redeploy</button><button class="btn btn--secondary">Rebuild &amp; Restore…</button><button class="btn btn--secondary">Promote…</button></div>
</div></div></div>`;

async function browser() {
  if (process.env.HARPOON_NO_BROWSER_TESTS === "1") return null;
  const require = createRequire(import.meta.url);
  let pw;
  for (const id of [process.env.PLAYWRIGHT_CORE_PATH, "playwright-core", `${homedir()}/.nvm/versions/node/v20.20.2/lib/node_modules/@playwright/cli/node_modules/playwright-core`].filter(Boolean)) {
    try { pw = require(id); break; } catch { /* try the next location */ }
  }
  assert.ok(pw, "playwright-core could not be loaded, so the layout cannot be measured (install it, or set HARPOON_NO_BROWSER_TESTS=1 to opt out)");
  return pw.chromium.launch({ channel: "chrome" });
}

test("the release card's markup in the layout check still matches Screens.tsx", () => {
  const hero = screens.slice(screens.indexOf('className="dp-panel dp-herocard"'), screens.indexOf('<div className="dp-tabs"'));
  for (const cls of ["dp-hero__state", "dp-actions dp-actions--end", "<PreviewThumb"]) assert.ok(hero.includes(cls), cls);
  assert.ok(hero.indexOf("<PreviewThumb") < hero.indexOf("dp-hero__state") && hero.indexOf("dp-hero__state") < hero.indexOf("dp-actions"));
});

test("the release card lays out by its own width: no overflow, readable details, controls usable, at every width", async (t) => {
  const b = await browser();
  if (!b) return t.skip("browser tests opted out");
  try {
    const page = await b.newPage();
    for (const thumb of [true, false]) {
      await page.setContent(`<style>body{margin:0;font:14px system-ui}.dp-root{container:dp/inline-size}${css}</style>${card(thumb)}`);
      for (const width of [1400, 1150, 1000, 800, 640, 600, 480, 360]) {
        await page.evaluate((w) => { document.querySelector(".dp-root").style.width = `${w}px`; }, width);
        const m = await page.evaluate(() => {
          const r = (sel) => { const e = document.querySelector(sel); if (!e) return null; const b = e.getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height, r: b.right, b: b.bottom }; };
          const root = document.querySelector(".dp-root");
          const buttons = [...document.querySelectorAll(".dp-actions button")].map((e) => e.getBoundingClientRect());
          const idEl = [...document.querySelectorAll(".dp-hero__state b")][0];
          return { overflowX: root.scrollWidth > root.clientWidth + 1, hero: r(".dp-hero"), thumb: r(".dp-thumb"), state: r(".dp-hero__state"), actions: r(".dp-actions"), url: r(".dp-address__url"), buttons, idLines: Math.round(idEl.getBoundingClientRect().height / (parseFloat(getComputedStyle(idEl).lineHeight) || parseFloat(getComputedStyle(idEl).fontSize) * 1.25)) };
        });
        const at = `thumb=${thumb} width=${width}`;
        assert.equal(m.overflowX, false, `no horizontal overflow (${at})`);
        assert.ok(m.state.w >= (width >= 640 ? 300 : 280), `details stay readable: ${Math.round(m.state.w)}px (${at})`);
        assert.ok(m.idLines <= 2, `the release id does not wrap character by character: ${m.idLines} lines (${at})`);
        assert.ok(m.url.h < 60, `the URL stays on a few lines: ${Math.round(m.url.h)}px tall (${at})`);
        for (const bt of m.buttons) assert.ok(bt.width >= 56 && bt.right <= m.hero.r + 1, `controls keep a usable size inside the card: ${Math.round(bt.width)}px wide, right edge ${Math.round(bt.right)} of ${Math.round(m.hero.r)} (${at})`);
        if (width >= 1100) assert.ok(m.actions.x >= m.state.r - 1 && Math.abs(m.actions.y - m.hero.y) < m.hero.h, `wide: actions sit beside the details (${at})`);
        else assert.ok(m.actions.y >= m.state.b - 1, `actions move below the details (${at})`);
        if (thumb && width >= 640) assert.ok(m.thumb.r <= m.state.x + 1 && m.thumb.y <= m.state.y + 4, `thumbnail beside the details (${at})`);
        if (thumb && width < 640) assert.ok(m.state.y >= m.thumb.b - 1, `narrow: details stack under the thumbnail (${at})`);
      }
    }
  } finally { await b.close(); }
});

test("the thumbnail keeps one size and a reachable Refresh in the active, loading and unavailable states", async (t) => {
  const b = await browser();
  if (!b) return t.skip("browser tests opted out");
  const px = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='960' height='600'><rect width='960' height='600' fill='white'/></svg>";
  const states = {
    active: `<button class="dp-thumb__img"><img alt="p" src="${px}"></button>`,
    loading: `<div class="dp-thumb__img dp-thumb__none" aria-busy="true"><span>Capturing the page…</span></div>`,
    unavailable: `<div class="dp-thumb__img dp-thumb__none"><b>Preview unavailable</b><span>${"This app's Content-Security-Policy includes upgrade-insecure-requests and more words ".repeat(6)}</span></div>`,
  };
  try {
    const page = await b.newPage();
    for (const [name, body] of Object.entries(states)) {
      await page.setContent(`<style>body{margin:0;font:14px system-ui}${css}</style><div style="width:300px"><div class="dp-thumb">${body}<button class="dp-thumb__refresh" aria-label="Refresh preview">↻</button></div></div>`);
      const m = await page.evaluate(() => {
        const t = document.querySelector(".dp-thumb").getBoundingClientRect();
        const img = document.querySelector(".dp-thumb__img").getBoundingClientRect();
        const r = document.querySelector(".dp-thumb__refresh").getBoundingClientRect();
        return { tw: t.width, iw: img.width, ih: img.height, rr: r.right, rt: r.top, ir: img.right, it: img.top, rw: r.width };
      });
      assert.equal(Math.round(m.iw), 220, name);
      assert.equal(Math.round(m.ih), 138, `${name}: 16:10 box, the message does not grow it`);
      assert.ok(m.rr <= m.ir + 0.5 && m.rt >= m.it - 0.5 && m.rw >= 24, `${name}: Refresh sits inside the thumbnail's corner`);
    }
  } finally { await b.close(); }
});
