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

const FLOAT_EPSILON = 0.001;
const WARN_TICK_LAG = 6;
const WARN_RENDER_DELTA = 0.75;
const SHARED_PLAYWRIGHT_PACKAGE = path.join(
  os.homedir(),
  ".codex",
  "playwright-runtime",
  "package.json",
);
const { chromium } = await loadPlaywright();

async function main() {
  const options = parseArgs(process.argv.slice(2));

  const browser = await chromium.launch({ headless: options.headless });
  const context = await browser.newContext({
    viewport: { width: 1400, height: 900 },
  });
  const page = await context.newPage();

  try {
    await page.goto(options.pageUrl, { waitUntil: "load" });
    await page.waitForSelector("canvas", { timeout: options.connectTimeoutMs });
    await waitForClientBridge(page, options.connectTimeoutMs);
    await ensureConnected(page, options.connectTimeoutMs);
    await focusCanvas(page);

    const sampler = collectSamples(page, options);
    await runScenario(page);
    const result = await sampler;

    const report = finalizeReport(result, options);
    await writeReport(report, options.reportPath);
    printSummary(report);

    if (report.summary.errorCount > 0) {
      process.exitCode = 1;
    }
  } finally {
    await context.close().catch(() => {});
    await browser.close().catch(() => {});
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
      const sharedRequire = createRequire(SHARED_PLAYWRIGHT_PACKAGE);
      return sharedRequire("playwright");
    } catch (sharedError) {
      throw new Error(
        `Could not load Playwright from the current Node module resolution path or the shared Codex runtime at ${SHARED_PLAYWRIGHT_PACKAGE}. Local error: ${localError}. Shared error: ${sharedError}`,
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

async function collectSamples(page, options) {
  const startedAt = Date.now();
  const rawSamples = [];
  const findings = [];
  const seen = new Set();
  const metrics = {
    sampleCount: 0,
    matchedSamples: 0,
    missingServerFrameSamples: 0,
    maxServerTickLag: 0,
    maxLocalRenderPredictionDelta: 0,
  };

  while (Date.now() - startedAt < options.scenarioMs) {
    const clientLatest = await readClientLatest(page);
    const serverExport = await readServerExport(options.serverDebugUrl);

    metrics.sampleCount += 1;
    rawSamples.push({
      atMs: Date.now() - startedAt,
      clientLatest,
      serverLatestTick: serverExport.latest?.tick ?? null,
    });

    analyzeSample(clientLatest, serverExport, findings, seen, metrics, rawSamples.length);

    await sleep(options.sampleMs);
  }

  return {
    rawSamples,
    findings,
    metrics,
  };
}

async function readClientLatest(page) {
  return page.evaluate(() => window.__discordiumDebugBridge?.latest ?? null);
}

async function readServerExport(serverDebugUrl) {
  const response = await fetch(serverDebugUrl);
  if (!response.ok) {
    throw new Error(`Failed to fetch server debug bridge: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

function analyzeSample(clientLatest, serverExport, findings, seen, metrics, sampleIndex) {
  if (!clientLatest) {
    pushFinding(findings, seen, {
      severity: "error",
      code: "missing_client_bridge_frame",
      message: "Client bridge did not expose a latest frame",
      sampleIndex,
    });
    return;
  }

  if (clientLatest.latest_server_tick === null || clientLatest.client_id === null) {
    pushFinding(findings, seen, {
      severity: "error",
      code: "client_not_connected",
      message: "Client bridge never reached a connected tick-bearing state",
      sampleIndex,
    });
    return;
  }

  const serverFrame = findServerClientFrame(
    serverExport.frames ?? [],
    clientLatest.latest_server_tick,
    clientLatest.client_id,
  );
  if (!serverFrame) {
    metrics.missingServerFrameSamples += 1;
    pushFinding(findings, seen, {
      severity: "error",
      code: "missing_server_frame_for_client_tick",
      message: `Server history does not contain tick ${clientLatest.latest_server_tick} for client ${clientLatest.client_id}`,
      sampleIndex,
      tick: clientLatest.latest_server_tick,
      clientId: clientLatest.client_id,
    });
    return;
  }

  metrics.matchedSamples += 1;

  const latestServerTick = serverExport.latest?.tick ?? clientLatest.latest_server_tick;
  const tickLag = latestServerTick - clientLatest.latest_server_tick;
  metrics.maxServerTickLag = Math.max(metrics.maxServerTickLag, tickLag);
  if (tickLag > WARN_TICK_LAG) {
    pushFinding(findings, seen, {
      severity: "warn",
      code: "client_tick_lag_high",
      message: `Client authoritative tick trails server by ${tickLag} ticks`,
      sampleIndex,
      tickLag,
    });
  }

  if (clientLatest.applied_world_tick !== clientLatest.latest_server_tick) {
    pushFinding(findings, seen, {
      severity: "error",
      code: "client_applied_tick_mismatch",
      message: `Client applied tick ${clientLatest.applied_world_tick} differs from latest received tick ${clientLatest.latest_server_tick}`,
      sampleIndex,
    });
  }

  compareScalar(
    findings,
    seen,
    sampleIndex,
    "phase",
    clientLatest.phase,
    serverFrame.world.phase,
  );
  compareScalar(
    findings,
    seen,
    sampleIndex,
    "wave",
    clientLatest.wave,
    serverFrame.world.wave,
  );
  compareScalar(
    findings,
    seen,
    sampleIndex,
    "team_life",
    clientLatest.team_life,
    serverFrame.world.team_life,
  );
  compareScalar(
    findings,
    seen,
    sampleIndex,
    "acked_input_seq",
    clientLatest.latest_acked_input_seq,
    serverFrame.world.your_last_input_seq,
  );

  compareSnapshotList(
    findings,
    seen,
    sampleIndex,
    "objectives",
    clientLatest.objectives,
    serverFrame.world.objectives,
    (item) => String(item.lane),
  );
  compareSnapshotList(
    findings,
    seen,
    sampleIndex,
    "heroes",
    clientLatest.authoritative_heroes,
    serverFrame.world.heroes,
    (item) => String(item.client_id),
  );
  compareSnapshotList(
    findings,
    seen,
    sampleIndex,
    "enemies",
    clientLatest.authoritative_enemies,
    serverFrame.world.enemies,
    (item) => String(item.id),
  );
  compareSnapshotList(
    findings,
    seen,
    sampleIndex,
    "towers",
    clientLatest.authoritative_towers,
    serverFrame.world.towers,
    (item) => String(item.id),
  );

  const localRender = clientLatest.rendered_actors.find((actor) => actor.kind === "LocalHero");
  const predictedLocal = clientLatest.predicted_heroes.find(
    (hero) => hero.client_id === clientLatest.client_id,
  );
  if (localRender && predictedLocal) {
    const delta = distance(localRender.pos, predictedLocal.pos);
    metrics.maxLocalRenderPredictionDelta = Math.max(
      metrics.maxLocalRenderPredictionDelta,
      delta,
    );
    if (delta > WARN_RENDER_DELTA) {
      pushFinding(findings, seen, {
        severity: "warn",
        code: "local_render_prediction_delta_high",
        message: `Local rendered hero is ${delta.toFixed(3)} units away from predicted hero`,
        sampleIndex,
        delta,
      });
    }
  }
}

function findServerClientFrame(frames, tick, clientId) {
  const frame = frames.find((item) => item.tick === tick);
  if (!frame) {
    return null;
  }
  return frame.clients.find((item) => item.client_id === clientId) ?? null;
}

function compareScalar(findings, seen, sampleIndex, label, clientValue, serverValue) {
  if (canonicalJson(clientValue) === canonicalJson(serverValue)) {
    return;
  }
  pushFinding(findings, seen, {
    severity: "error",
    code: `scalar_mismatch_${label}`,
    message: `Client ${label} does not match server authoritative value`,
    sampleIndex,
    clientValue,
    serverValue,
  });
}

function compareSnapshotList(
  findings,
  seen,
  sampleIndex,
  label,
  clientItems,
  serverItems,
  keyFor,
) {
  const clientMap = new Map(clientItems.map((item) => [keyFor(item), item]));
  const serverMap = new Map(serverItems.map((item) => [keyFor(item), item]));
  const keys = new Set([...clientMap.keys(), ...serverMap.keys()]);

  for (const key of keys) {
    const clientItem = clientMap.get(key);
    const serverItem = serverMap.get(key);

    if (!clientItem || !serverItem) {
      pushFinding(findings, seen, {
        severity: "error",
        code: `missing_${label}_${key}`,
        message: `${label} entry ${key} is not present on both client and server`,
        sampleIndex,
        clientPresent: !!clientItem,
        serverPresent: !!serverItem,
      });
      continue;
    }

    if (canonicalJson(clientItem) === canonicalJson(serverItem)) {
      continue;
    }

    pushFinding(findings, seen, {
      severity: "error",
      code: `snapshot_mismatch_${label}_${key}`,
      message: `${label} entry ${key} differs from server authoritative state`,
      sampleIndex,
      clientItem: canonicalize(clientItem),
      serverItem: canonicalize(serverItem),
    });
  }
}

function canonicalize(value) {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      return value;
    }
    if (Math.abs(value - Math.round(value)) <= FLOAT_EPSILON) {
      return Math.round(value);
    }
    return Number(value.toFixed(4));
  }
  if (Array.isArray(value)) {
    return value.map((item) => canonicalize(item));
  }
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((acc, key) => {
        acc[key] = canonicalize(value[key]);
        return acc;
      }, {});
  }
  return value;
}

function canonicalJson(value) {
  return JSON.stringify(canonicalize(value));
}

function pushFinding(findings, seen, finding) {
  const key = canonicalJson({
    code: finding.code,
    message: finding.message,
    clientValue: finding.clientValue,
    serverValue: finding.serverValue,
    clientItem: finding.clientItem,
    serverItem: finding.serverItem,
    tick: finding.tick,
    clientId: finding.clientId,
  });
  if (seen.has(key)) {
    return;
  }
  seen.add(key);
  findings.push(finding);
}

function finalizeReport(result, options) {
  const errorCount = result.findings.filter((finding) => finding.severity === "error").length;
  const warnCount = result.findings.filter((finding) => finding.severity === "warn").length;

  return {
    generatedAt: new Date().toISOString(),
    options,
    summary: {
      sampleCount: result.metrics.sampleCount,
      matchedSamples: result.metrics.matchedSamples,
      missingServerFrameSamples: result.metrics.missingServerFrameSamples,
      maxServerTickLag: result.metrics.maxServerTickLag,
      maxLocalRenderPredictionDelta: Number(
        result.metrics.maxLocalRenderPredictionDelta.toFixed(4),
      ),
      errorCount,
      warnCount,
    },
    findings: result.findings,
    samples: result.rawSamples,
  };
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

function distance(a, b) {
  const dx = a[0] - b[0];
  const dy = a[1] - b[1];
  return Math.hypot(dx, dy);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch((error) => {
  console.error(error?.stack ?? String(error));
  process.exit(1);
});
