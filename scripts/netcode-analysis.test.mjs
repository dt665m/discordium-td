import { test } from 'node:test';
import assert from 'node:assert/strict';
import { analyzeSample, finalizeReport } from './netcode-analysis.mjs';
function fixture() {
  const world = {phase:'Playing', wave:1, team_life:10,  objectives:[], heroes:[], enemies:[], towers:[]};
  return {
    client: {connected:true, frame_index:1, latest_server_tick:10, client_id:1, applied_world_tick:10, phase:'Playing', wave:1, team_life:10, latest_acked_input_seq:null, objectives:[], authoritative_heroes:[], authoritative_enemies:[], authoritative_towers:[], rendered_actors:[], predicted_heroes:[]},
    server: {latest:{tick:10}, frames:[{tick:10, clients:[{client_id:1,world}]}]},
    result: {rawSamples:[],findings:[],metrics:{sampleCount:0,matchedSamples:0,missingServerFrameSamples:0,maxServerTickLag:0,maxLocalRenderPredictionDelta:0}},
  };
}
test('disconnected snapshots cannot pass as matched', () => {
  const {client,server,result} = fixture(); client.connected = false;
  analyzeSample(client,server,result.findings,new Set(),result.metrics,1,0);
  assert.equal(result.metrics.matchedSamples,0);
  assert.equal(result.findings[0].code,'client_not_connected');
});
test('frozen client and server produce stalled errors', () => {
  const {client,server,result} = fixture(); const seen = new Set();
  analyzeSample(client,server,result.findings,seen,result.metrics,1,0);
  analyzeSample(client,server,result.findings,seen,result.metrics,2,1600);
  assert.deepEqual(result.findings.map(f=>f.code),['clientFrame_stalled','serverTick_stalled']);
});
test('empty reports fail coverage', () => {
  const {result} = fixture();
  assert.equal(finalizeReport(result,{}).summary.errorCount,1);
});
test('advancing matched state passes', () => {
  const {client,server,result} = fixture(); const seen = new Set();
  analyzeSample(client,server,result.findings,seen,result.metrics,1,0);
  client.frame_index++; client.latest_server_tick++; client.applied_world_tick++;
  server.latest.tick++; server.frames[0].tick++;
  analyzeSample(client,server,result.findings,seen,result.metrics,2,150);
  assert.equal(finalizeReport(result,{}).summary.errorCount,0);
});
test('matching authority cannot hide excessive prediction lead', () => {
  const {client,server,result} = fixture(); client.predicted_tick = client.latest_server_tick + 16;
  analyzeSample(client,server,result.findings,new Set(),result.metrics,1,0);
  assert.ok(result.findings.some(f => f.code === 'unbounded_prediction_lead'));
});
test('remote teleport cannot hide behind matching authoritative snapshots', () => {
  const {client,server,result} = fixture(); const seen = new Set();
  client.capture_elapsed_ms = 0;
  client.rendered_actors = [{kind:'RemoteHero',id:2,pos:[0,0]}];
  analyzeSample(client,server,result.findings,seen,result.metrics,1,0);
  client.capture_elapsed_ms = 100; client.frame_index++;
  client.rendered_actors[0].pos = [10,0];
  analyzeSample(client,server,result.findings,seen,result.metrics,2,100);
  assert.ok(result.findings.some(f => f.code === 'remote_presentation_jump'));
});
