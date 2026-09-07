const FLOAT_EPSILON = 0.001;
const WARN_TICK_LAG = 6;
const WARN_RENDER_DELTA = 0.75;

export function analyzeSample(clientLatest, serverExport, findings, seen, metrics, sampleIndex, nowMs = Date.now()) {
  if (!clientLatest) {
    pushFinding(findings, seen, {
      severity: "error",
      code: "missing_client_bridge_frame",
      message: "Client bridge did not expose a latest frame",
      sampleIndex,
    });
    return;
  }

  if (!clientLatest.connected || clientLatest.latest_server_tick === null || clientLatest.client_id === null) {
    pushFinding(findings, seen, {
      severity: "error",
      code: "client_not_connected",
      message: "Client bridge never reached a connected tick-bearing state",
      sampleIndex,
    });
    return;
  }

  for (const [key, value] of [["clientSession", clientLatest.session_id], ["serverSession", serverExport.latest?.identity?.process_session]]) {
    if (metrics[key] !== undefined && metrics[key] !== value) {
      pushFinding(findings, seen, {severity: "error", code: `${key}_changed`, message: `${key} changed during verification`, sampleIndex});
    }
    metrics[key] = value;
  }
  for (const [key, value] of [["clientFrame", clientLatest.frame_index], ["serverTick", serverExport.latest?.tick]]) {
    if (metrics[key] !== value) {
      metrics[key] = value;
      metrics[`${key}ProgressMs`] = nowMs;
      metrics[`${key}Changes`] = (metrics[`${key}Changes`] ?? 0) + 1;
    } else if (nowMs - metrics[`${key}ProgressMs`] > 1500) {
      pushFinding(findings, seen, {severity: "error", code: `${key}_stalled`, message: `${key} has not advanced for 1.5 seconds`, sampleIndex});
    }
  }
  if (clientLatest.snapshot_age_ms > 1500) {
    pushFinding(findings, seen, {severity: "error", code: "stale_snapshot", message: "Last snapshot is more than 1.5 seconds old", sampleIndex});
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

export function finalizeReport(result, options) {
  if (result.metrics.matchedSamples < 2 || (result.metrics.clientFrameChanges ?? 0) < 2 || (result.metrics.serverTickChanges ?? 0) < 2) {
    result.findings.push({severity: "error", code: "insufficient_live_coverage", message: "Need at least two matched samples with advancing client frames and server ticks"});
  }
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
    browserLogs: result.browserLogs ?? [],
  };
}

function distance(a, b) {
  const dx = a[0] - b[0];
  const dy = a[1] - b[1];
  return Math.hypot(dx, dy);
}
