import test from 'node:test';
import assert from 'node:assert/strict';
import {
  BEAV_BRIDGE_SUPPORTED_ACTIONS,
  BEAV_BRIDGE_SUPPORTED_VIEWS,
  isSupportedBeavBridgeAction,
  isSupportedBeavBridgeView,
  normalizeBridgePayload,
  requireBeavBridgeAction,
  requireBeavBridgeView,
} from '../beavLocalBridgeContract.ts';

test('beav bridge contract lists all MCP-addressable operation workbench views', () => {
  assert.deepEqual([...BEAV_BRIDGE_SUPPORTED_VIEWS].sort(), [
    'archives',
    'chat',
    'cover-studio',
    'generation-studio',
    'knowledge',
    'manuscripts',
    'media-library',
    'redclaw',
    'settings',
    'skills',
    'subjects',
    'team',
    'wander',
    'workboard',
  ].sort());
});

test('beav bridge contract lists all bridge actions used by Beav MCP', () => {
  assert.deepEqual(BEAV_BRIDGE_SUPPORTED_ACTIONS, [
    'app_cli',
    'navigate',
    'tool_call',
  ]);
  assert.equal(isSupportedBeavBridgeAction('app_cli'), true);
  assert.equal(isSupportedBeavBridgeAction('not-an-action'), false);
  assert.equal(requireBeavBridgeAction('tool_call'), 'tool_call');
  assert.throws(() => requireBeavBridgeAction(''), /Unsupported Beav bridge action/);
});

test('beav bridge contract validates view ids', () => {
  assert.equal(isSupportedBeavBridgeView('redclaw'), true);
  assert.equal(isSupportedBeavBridgeView('not-a-view'), false);
  assert.equal(requireBeavBridgeView('subjects'), 'subjects');
  assert.throws(() => requireBeavBridgeView('not-a-view'), /Unsupported Beav view/);
});

test('beav bridge contract normalizes payload objects only', () => {
  const payload = { command: 'spaces list' };
  assert.equal(normalizeBridgePayload(payload), payload);
  assert.deepEqual(normalizeBridgePayload(null), {});
  assert.deepEqual(normalizeBridgePayload(['bad']), {});
});
