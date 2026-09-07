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
  scenarioMs: 5000,
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
  let browser, context, sampler;
  const failure = (error) => result.findings.push({severity: "error", code: "verifier_failure", message: error?.stack ?? String(error)});
  try {
    const { chromium } = await loadPlaywright();
    browser = await chromium.launch({ headless: options.headless });
    context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
    const page = await context.newPage();
    const log = (kind, message) => {
      if (result.browserLogs.length >= 200) result.browserLogs.shift();
      result.browserLogs.push({at: new Date().toISOString(), kind, message});
    };
    page.on("console", (message) => log(message.type(), message.text()));
    page.on("pageerror", (error) => log("pageerror", String(error)));
    page.on("requestfailed", (request) => log("requestfailed", `${request.url()}: ${request.failure()?.errorText}`));
    await page.goto(options.pageUrl, { waitUntil: "load" });
    await page.waitForSelector("canvas", { timeout: options.connectTimeoutMs });
    await waitForClientBridge(page, options.connectTimeoutMs);
    await ensureConnected(page, options.connectTimeoutMs);
    await focusCanvas(page);
    sampler = collectSamples(page, options, result).catch(failure);
    await runScenario(page);
    await sampler;
  } catch (error) { failure(error); }
  finally {
    options.stopSampling = true;
    await sampler;
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
  return path.join("target", "netcode-verifier", dateDir, runId, "report.json");
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

async function runScenario(page) {
  await holdKey(page, "w", 900);
  await holdKey(page, "d", 700);
  await page.keyboard.press("j");
  await holdKey(page, "s", 700);
  await holdKey(page, " ", 450);
  await page.keyboard.press("k");
  await holdKey(page, "a", 700);
  await page.waitForTimeout(900);
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
