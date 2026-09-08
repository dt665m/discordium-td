#!/usr/bin/env node

import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const DEFAULTS = {
  pageUrl: "http://127.0.0.1:1420/?debug_bridge=1",
  serverDebugUrl: "http://127.0.0.1:8080/debug/bridge",
  sampleMs: 150,
  scenarioMs: 15000,
  clients: 2,
  stress: false,
  conditionClient: false,
  deviceScaleFactor: process.platform === "darwin" ? 2 : 1,
  connectTimeoutMs: 30000,
  reportPath: buildDefaultReportPath(),
  headless: false,
};

import { analyzeSample, finalizeReport } from "./netcode-analysis.mjs";
const PLAYWRIGHT_RUNTIME_DIR =
  process.env.PLAYWRIGHT_RUNTIME_DIR ??
  path.join(os.tmpdir(), "discordium-td-playwright-runtime");
const FALLBACK_PLAYWRIGHT_PACKAGE = path.join(
  PLAYWRIGHT_RUNTIME_DIR,
  "package.json",
);


async function main() {
  const options = parseArgs(process.argv.slice(2));
  const result = { rawSamples: [], findings: [], browserLogs: [], metrics: {
    sampleCount: 0, matchedSamples: 0, missingServerFrameSamples: 0,
    maxServerTickLag: 0, maxLocalRenderPredictionDelta: 0,
  }};
  let browser, context;
  const samplers = [];
  const failure = (error) => result.findings.push({severity: "error", code: "verifier_failure", message: error?.stack ?? String(error)});
  try {
    const { chromium } = await loadPlaywright();
    browser = await chromium.launch({ headless: options.headless, args: ["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows"] });
    context = await browser.newContext({ viewport: { width: 1400, height: 900 }, deviceScaleFactor: options.deviceScaleFactor });
    const pages = [];
    const runs = [];
    for (let index = 0; index < options.clients; index++) {
      const page = await context.newPage();
      const run = { rawSamples: [], findings: [], metrics: { sampleCount: 0, matchedSamples: 0,
        missingServerFrameSamples: 0, maxServerTickLag: 0, maxLocalRenderPredictionDelta: 0 }};
      const log = (kind, message) => {
        if (result.browserLogs.length >= 200) result.browserLogs.shift();
        result.browserLogs.push({at: new Date().toISOString(), client: index, kind, message});
      };
      page.on("console", message => log(message.type(), message.text()));
      page.on("pageerror", error => log("pageerror", String(error)));
      await page.goto(options.pageUrl, { waitUntil: "load" });
      await page.waitForSelector("canvas", { timeout: options.connectTimeoutMs });
      await waitForClientBridge(page, options.connectTimeoutMs);
      await ensureConnected(page, options.connectTimeoutMs);
      await focusCanvas(page);
      if (options.conditionClient) await page.evaluate(() => { window.__discordiumDebugBridgeCommand = "network_300ms"; });
      pages.push(page); runs.push(run);
    }
    for (let index = 0; index < pages.length; index++) {
      samplers.push(collectSamples(pages[index], options, runs[index]).catch(failure));
    }
    await Promise.all(pages.map((page, index) => runScenario(page, options.scenarioMs, index, options.stress)));
    await Promise.all(samplers);
    for (let index = 0; index < runs.length; index++) {
      const run = runs[index];
      if (options.clients > 1 && !(run.metrics.remoteMovementSamples > 0)) {
        run.findings.push({severity: "error", code: "missing_remote_movement_coverage", message: "No moving remote hero was observed"});
      }
      const report = finalizeReport(run, options);
      result.rawSamples.push(...run.rawSamples.map(sample => ({...sample, clientIndex: index})));
      result.findings.push(...report.findings.map(finding => ({...finding, clientIndex: index})));
      for (const key of ["sampleCount", "matchedSamples", "missingServerFrameSamples", "clientFrameChanges", "serverTickChanges", "remoteMovementSamples"]) {
        result.metrics[key] = (result.metrics[key] ?? 0) + (run.metrics[key] ?? 0);
      }
      for (const key of ["maxServerTickLag", "maxLocalRenderPredictionDelta", "maxPredictionLeadTicks", "maxInputAckAgeMs", "maxRemoteStep"]) {
        result.metrics[key] = Math.max(result.metrics[key] ?? 0, run.metrics[key] ?? 0);
      }
    }
  } catch (error) { failure(error); }
  finally {
    options.stopSampling = true;
    await Promise.all(samplers);
    delete options.stopSampling;
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
    const report = finalizeReport(result, options);
    await writeReport(report, options.reportPath);
    printSummary(report);
    if (report.summary.errorCount > 0) process.exitCode = 1;
  }
}

function parseArgs(argv) {
  const options = { ...DEFAULTS };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = argv[i + 1];

    if (arg === "--page-url" && next) {
      options.pageUrl = next;
      i += 1;
      continue;
    }
    if (arg === "--server-debug-url" && next) {
      options.serverDebugUrl = next;
      i += 1;
      continue;
    }
    if (arg === "--sample-ms" && next) {
      options.sampleMs = Number(next);
      i += 1;
      continue;
    }
    if (arg === "--scenario-ms" && next) {
      options.scenarioMs = Number(next);
      i += 1;
      continue;
    }
    if (arg === "--connect-timeout-ms" && next) {
      options.connectTimeoutMs = Number(next);
      i += 1;
      continue;
    }
    if (arg === "--report" && next) {
      options.reportPath = next;
      i += 1;
      continue;
    }
    if (["--clients", "--device-scale-factor"].includes(arg) && next) {
      options[arg === "--clients" ? "clients" : "deviceScaleFactor"] = Number(next); i++; continue;
    }
    if (arg === "--condition-client") { options.conditionClient = true; continue; }
    if (arg === "--stress") { options.stress = true; continue; }
    if (arg === "--headless") {
      options.headless = true;
      continue;
    }
    if (arg === "--headed") {
      options.headless = false;
      continue;
    }
    if (arg === "--help") {
      printHelp();
      process.exit(0);
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  for (const key of ["sampleMs", "scenarioMs", "connectTimeoutMs"]) {
    if (!Number.isFinite(options[key]) || options[key] <= 0) throw new Error(`${key} must be a positive finite number`);
  }
  if (!Number.isInteger(options.clients) || options.clients < 1 || options.clients > 8) throw new Error("clients must be 1–8");
  if (![1, 2].includes(options.deviceScaleFactor)) throw new Error("device-scale-factor must be 1 or 2");
  return options;
}

function printHelp() {
  console.log(`Usage: node ./scripts/netcode-verify.mjs [options]

Options:
  --page-url URL
  --server-debug-url URL
  --sample-ms N
  --scenario-ms N
  --connect-timeout-ms N
  --report PATH
  --clients N
  --stress
  --condition-client
  --device-scale-factor N
  --headless
  --headed
  --help`);
}

async function loadPlaywright() {
  try {
    return await import("playwright");
  } catch (localError) {
    try {
      const fallbackRequire = createRequire(FALLBACK_PLAYWRIGHT_PACKAGE);
      return fallbackRequire("playwright");
    } catch (fallbackError) {
      throw new Error(
        `Could not load Playwright from the current Node module resolution path or the temporary runtime at ${FALLBACK_PLAYWRIGHT_PACKAGE}. Run ./scripts/run-netcode-verify.sh to prepare it. Local error: ${localError}. Fallback error: ${fallbackError}`,
      );
    }
  }
}

function buildDefaultReportPath() {
  const now = new Date();
  const dateDir = now.toISOString().slice(0, 10);
  const runId = `run-${now
    .toISOString()
    .replaceAll(":", "-")
    .replaceAll(".", "-")}-${process.pid}`;
  return path.join("target", "reports", "netcode-verifier", dateDir, runId, "report.json");
}

async function waitForClientBridge(page, timeoutMs) {
  await page.waitForFunction(() => {
    return !!window.__discordiumDebugBridge?.enabled;
  }, null, { timeout: timeoutMs });
}

async function ensureConnected(page, timeoutMs) {
  const latest = await readClientLatest(page);
  if (latest?.connected && latest.latest_server_tick !== null) {
    return;
  }

  await page.evaluate(() => {
    window.__discordiumDebugBridgeCommand = "connect_dev";
  });

  await page.waitForFunction(() => {
    const latest = window.__discordiumDebugBridge?.latest;
    return !!latest && latest.connected && latest.latest_server_tick !== null;
  }, null, { timeout: timeoutMs });
}

async function focusCanvas(page) {
  const box = await page.locator("canvas").boundingBox();
  if (!box) {
    throw new Error("Canvas bounding box not available");
  }
  await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.5);
}

async function runScenario(page, durationMs, index, stress) {
  let interrupted = false;
  const started = Date.now();
  const keys = index % 2 ? ["d", "s", "a", "w"] : ["w", "d", "s", "a"];
  while (Date.now() - started < durationMs) {
    for (const key of keys) {
      if (Date.now() - started >= durationMs) break;
      await holdKey(page, key, 900);
    }
    if (stress && index === 0 && !interrupted) {
      interrupted = true;
      await page.evaluate(() => { window.__discordiumDebugBridgeCommand = "network_outage"; });
      await holdKey(page, "d", 1200);
      await page.evaluate(() => { const until = performance.now() + 150; while (performance.now() < until) {} });
    }
    await page.keyboard.press("j");
    await page.keyboard.press("k");
  }
}

async function holdKey(page, key, durationMs) {
  await page.keyboard.down(key);
  await page.waitForTimeout(durationMs);
  await page.keyboard.up(key);
}

async function collectSamples(page, options, result) {
  const startedAt = Date.now();
  const { rawSamples, findings, metrics } = result;
  const seen = new Set();
  while (!options.stopSampling && Date.now() - startedAt < options.scenarioMs) {
    const clientLatest = await readClientLatest(page);
    const serverExport = await readServerExport(options.serverDebugUrl);
    metrics.sampleCount += 1;
    rawSamples.push({ atMs: Date.now() - startedAt, clientLatest,
      serverLatestTick: serverExport.latest?.tick ?? null,
      serverIdentity: serverExport.latest?.identity ?? null });
    analyzeSample(clientLatest, serverExport, findings, seen, metrics, rawSamples.length);
    await sleep(options.sampleMs);
  }
}

async function readClientLatest(page) {
  return page.evaluate(() => window.__discordiumDebugBridge?.latest ?? null);
}

async function readServerExport(serverDebugUrl) {
  const response = await fetch(serverDebugUrl, { signal: AbortSignal.timeout(3000) });
  if (!response.ok) {
    throw new Error(`Failed to fetch server debug bridge: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

async function writeReport(report, reportPath) {
  await mkdir(path.dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
}

function printSummary(report) {
  console.log(`Netcode verifier report: ${report.options.reportPath}`);
  console.log(
    `Samples=${report.summary.sampleCount} matched=${report.summary.matchedSamples} errors=${report.summary.errorCount} warnings=${report.summary.warnCount}`,
  );
  console.log(
    `Max server tick lag=${report.summary.maxServerTickLag} max local render delta=${report.summary.maxLocalRenderPredictionDelta}`,
  );
  for (const finding of report.findings.slice(0, 12)) {
    console.log(`[${finding.severity}] ${finding.code}: ${finding.message}`);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch((error) => {
  console.error(error?.stack ?? String(error));
  process.exit(1);
});
