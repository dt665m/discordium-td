#!/usr/bin/env node

// Real browser/transport smoke test; start the dedicated server and Trunk first.
// Set PLAYWRIGHT_RUNTIME_DIR to a directory with Playwright installed.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";

const runtime = process.env.PLAYWRIGHT_RUNTIME_DIR
  ?? path.join(os.tmpdir(), "discordium-td-playwright-runtime");
const require = createRequire(path.join(runtime, "package.json"));
const { chromium } = require("playwright");
const base = process.argv[2] ?? "http://127.0.0.1:1420";
const output = path.resolve(process.argv[3] ?? "target/web-smoke");
await mkdir(output, { recursive: true });

const browser = await chromium.launch({
  headless: true,
  args: [
    "--use-angle=metal", "--ignore-gpu-blocklist",
    "--autoplay-policy=no-user-gesture-required",
    "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
  ],
});
const logs = [];
const errors = [];
const clients = [];
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1,
  });
  for (const renderer of ["graphics", "gizmos"]) {
    const page = await context.newPage();
    const client = { renderer, page, states: [] };
    clients.push(client);
    page.on("pageerror", error => errors.push({ renderer, message: String(error) }));
    page.on("console", message => {
      const text = message.text();
      logs.push({ renderer, level: message.type(), text });
      if (message.type() === "error") errors.push({ renderer, message: text });
      if (text.includes("Dreamwake browser playtest:")) {
        client.states.push(text);
        console.log(renderer, text);
      }
    });
    await page.goto(base, { waitUntil: "domcontentloaded" });
  }

  // Bevy renders its UI into the canvas. These coordinates target the first two
  // F6 controls at the fixed viewport above, using the same menu as a player.
  for (const client of clients) {
    await client.page.waitForSelector("canvas");
    await client.page.waitForTimeout(5_000);
    await client.page.keyboard.press("F6");
    await client.page.waitForTimeout(500);
    if (client.renderer === "gizmos") {
      await client.page.mouse.click(1000, 40);
    }
    await client.page.mouse.click(1000, 84);
    await client.page.keyboard.press("F6");
  }

  const deadline = Date.now() + 120_000;
  const connected = client => client.states.some(state => /phase=Combat.*party=2/.test(state));
  while (!clients.every(connected) && !errors.length && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  assert.equal(errors.length, 0, JSON.stringify(errors, null, 2));
  assert(clients.every(connected), "Both clients must reach authoritative combat with two players");

  // Allow an encounter to advance so screenshots exercise actors and effects,
  // rather than merely proving that a menu can render.
  await new Promise(resolve => setTimeout(resolve, 8_000));
  for (const client of clients) {
    await client.page.screenshot({ path: path.join(output, `${client.renderer}.png`) });
  }
  await clients[0].page.keyboard.press("F6");
  await new Promise(resolve => setTimeout(resolve, 500));
  await clients[0].page.screenshot({ path: path.join(output, "diagnostics.png") });
  assert.equal(errors.length, 0, JSON.stringify(errors, null, 2));
  console.log("Browser smoke passed: two clients share a WebRTC game with runtime gizmo controls.");
} finally {
  await writeFile(path.join(output, "browser-log.json"), JSON.stringify({ logs, errors }, null, 2));
  await browser.close();
}
