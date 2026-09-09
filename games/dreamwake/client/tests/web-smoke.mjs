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
const server = process.argv[3] ?? "http://127.0.0.1:8080";
const output = path.resolve(process.argv[4] ?? "target/web-smoke");
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
  for (const renderer of ["prototype", "legacy"]) {
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
    const url = new URL(base);
    url.searchParams.set("server", server);
    url.searchParams.set("autoplay", "1");
    url.searchParams.set("waitPlayers", "2");
    url.searchParams.set("renderer", renderer);
    await page.goto(url.toString(), { waitUntil: "domcontentloaded" });
  }

  const deadline = Date.now() + 120_000;
  const connected = client => client.states.some(state => /phase=Combat.*party=2/.test(state));
  while (!clients.every(connected) && !errors.length && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  assert.equal(errors.length, 0, JSON.stringify(errors, null, 2));
  assert(clients.every(connected), "Both renderers must reach authoritative combat with two players");

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
  console.log("Browser smoke passed: prototype + legacy renderers share a two-player WebRTC game.");
} finally {
  await writeFile(path.join(output, "browser-log.json"), JSON.stringify({ logs, errors }, null, 2));
  await browser.close();
}
