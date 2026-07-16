import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import {
  GooseBridgeService,
  GooseBridgeTaskCancelledError,
  GooseBridgeTaskQueue,
  GooseChatEventAdapter,
  GooseSseParser,
  buildGooseHeaders,
  buildGooseReplyBody,
  buildGooseSidecarCommand,
  createGooseUserTextMessage,
  normalizeGooseSseFrame,
  selectGooseReplyEndpoint,
  splitGooseOpenAiCompatibleEndpoint,
} from '../index.ts';

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

test('builds goosed agent sidecar command and env', () => {
  const command = buildGooseSidecarCommand({
    executablePath: '/opt/goose/bin/goosed',
    host: '127.0.0.1',
    port: 31987,
    tls: false,
    secretKey: 'secret',
    cwd: '/work',
    gooseHome: '/tmp/goose-home',
    env: {
      OPENAI_API_KEY: 'test-key',
      OMITTED: undefined,
    },
    addBinaryDirToPath: true,
  });

  assert.equal(command.command, '/opt/goose/bin/goosed');
  assert.deepEqual(command.args, ['agent']);
  assert.equal(command.cwd, '/work');
  assert.equal(command.env.GOOSE_HOST, '127.0.0.1');
  assert.equal(command.env.GOOSE_PORT, '31987');
  assert.equal(command.env.GOOSE_TLS, 'false');
  assert.equal(command.env.GOOSE_SERVER__SECRET_KEY, 'secret');
  assert.equal(command.env.GOOSE_HOME, '/tmp/goose-home');
  assert.equal(command.env.OPENAI_API_KEY, 'test-key');
  assert.equal('OMITTED' in command.env, false);
  assert.match(command.env[process.platform === 'win32' ? 'Path' : 'PATH'], /\/opt\/goose\/bin/);
});

test('builds Goose headers with X-Secret-Key', () => {
  assert.deepEqual(buildGooseHeaders({ secretKey: 'secret' }), {
    'Content-Type': 'application/json',
    'X-Secret-Key': 'secret',
  });
});

test('splits OpenAI-compatible endpoint host and base path for Goose', () => {
  assert.deepEqual(splitGooseOpenAiCompatibleEndpoint('https://api.example.com/v1'), {
    host: 'https://api.example.com',
    basePath: 'v1/chat/completions',
    baseUrl: 'https://api.example.com/v1',
  });
  assert.deepEqual(splitGooseOpenAiCompatibleEndpoint('https://api.example.com/v1/chat/completions'), {
    host: 'https://api.example.com',
    basePath: 'v1/chat/completions',
    baseUrl: 'https://api.example.com/v1/chat/completions',
  });
  assert.deepEqual(splitGooseOpenAiCompatibleEndpoint('https://gateway.example.com/proxy/openai/v1/chat/completions'), {
    host: 'https://gateway.example.com/proxy/openai',
    basePath: 'v1/chat/completions',
    baseUrl: 'https://gateway.example.com/proxy/openai/v1/chat/completions',
  });
  assert.deepEqual(splitGooseOpenAiCompatibleEndpoint('https://gateway.example.com/proxy/openai/v1/responses'), {
    host: 'https://gateway.example.com/proxy/openai',
    basePath: 'v1/responses',
    baseUrl: 'https://gateway.example.com/proxy/openai/v1/responses',
  });
  assert.deepEqual(splitGooseOpenAiCompatibleEndpoint('http://127.0.0.1:3001/custom'), {
    host: 'http://127.0.0.1:3001/custom',
    basePath: 'v1/chat/completions',
    baseUrl: 'http://127.0.0.1:3001/custom',
  });
});

test('parses SSE Message, Error, Finish and Ping events', () => {
  const parser = new GooseSseParser();
  const frames = parser.push([
    'id: 1',
    'data: {"type":"Message","chat_request_id":"chat-1","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]},"token_state":{"total_tokens":3}}',
    '',
    'data: {"type":"Error","error":"boom"}',
    '',
    ': ping 0',
    '',
    'data: {"type":"Finish","reason":"stop","token_state":{"total_tokens":4}}',
    '',
    '',
  ].join('\n'));

  assert.equal(frames.length, 4);

  const message = normalizeGooseSseFrame(frames[0]);
  assert.equal(message.kind, 'message');
  assert.equal(message.chatRequestId, 'chat-1');
  assert.equal(message.message.role, 'assistant');
  assert.equal(message.tokenState?.total_tokens, 3);

  const error = normalizeGooseSseFrame(frames[1]);
  assert.equal(error.kind, 'error');
  assert.equal(error.error, 'boom');

  const ping = normalizeGooseSseFrame(frames[2]);
  assert.equal(ping.kind, 'ping');

  const finish = normalizeGooseSseFrame(frames[3]);
  assert.equal(finish.kind, 'finish');
  assert.equal(finish.reason, 'stop');
  assert.equal(finish.tokenState?.total_tokens, 4);
});

test('selects session reply endpoint when session events are enabled', () => {
  const endpoint = selectGooseReplyEndpoint(
    {
      host: '127.0.0.1',
      port: 3007,
      tls: false,
      secretKey: 'secret',
    },
    'session/a',
  );

  assert.equal(endpoint.kind, 'session');
  assert.equal(endpoint.replyUrl, 'http://127.0.0.1:3007/sessions/session%2Fa/reply');
  assert.equal(endpoint.eventsUrl, 'http://127.0.0.1:3007/sessions/session%2Fa/events');
  assert.equal(endpoint.cancelUrl, 'http://127.0.0.1:3007/sessions/session%2Fa/cancel');

  const body = buildGooseReplyBody({
    sessionId: 'session/a',
    requestId: '00000000-0000-0000-0000-000000000001',
    message: createGooseUserTextMessage('hello', 1),
  }, endpoint);

  assert.deepEqual(body, {
    request_id: '00000000-0000-0000-0000-000000000001',
    user_message: createGooseUserTextMessage('hello', 1),
  });
});

test('falls back to legacy reply endpoint without a session id or when disabled', () => {
  assert.deepEqual(selectGooseReplyEndpoint({ baseUrl: 'http://goose.local' }), {
    kind: 'legacy',
    method: 'POST',
    replyUrl: 'http://goose.local/reply',
  });
  assert.deepEqual(selectGooseReplyEndpoint({ baseUrl: 'http://goose.local', useSessionEvents: false }, 's1'), {
    kind: 'legacy',
    method: 'POST',
    replyUrl: 'http://goose.local/reply',
  });
});

test('task queue preserves concurrent enqueue order', async () => {
  const queue = new GooseBridgeTaskQueue();
  const order: string[] = [];

  const first = queue.enqueue('first', async () => {
    await wait(20);
    order.push('first');
    return 1;
  });
  const second = queue.enqueue('second', async () => {
    order.push('second');
    return 2;
  });
  const third = queue.enqueue('third', async () => {
    order.push('third');
    return 3;
  });

  assert.deepEqual(await Promise.all([first, second, third]), [1, 2, 3]);
  assert.deepEqual(order, ['first', 'second', 'third']);
  assert.deepEqual(queue.snapshot().map((item) => item.status), ['completed', 'completed', 'completed']);
});

test('task queue cancels queued and active work predictably', async () => {
  const queue = new GooseBridgeTaskQueue();
  const started: string[] = [];

  const active = queue.enqueue('active', async (signal) => {
    started.push('active');
    await new Promise((_resolve, reject) => {
      signal.addEventListener('abort', () => reject(new Error('aborted')), { once: true });
    });
  });
  const queued = queue.enqueue('queued', async () => {
    started.push('queued');
  });

  assert.equal(queue.cancel('queued'), true);
  await assert.rejects(queued, GooseBridgeTaskCancelledError);
  assert.equal(queue.get('queued')?.status, 'cancelled');

  assert.equal(queue.cancel('active'), true);
  await assert.rejects(active, GooseBridgeTaskCancelledError);
  assert.equal(queue.get('active')?.status, 'cancelled');
  assert.deepEqual(started, ['active']);
});

test('service starts and stops goosed sidecar with configured env', () => {
  const children: any[] = [];
  const fakeSpawn = ((command: string, args: string[], options: Record<string, unknown>) => {
    const child = new EventEmitter() as EventEmitter & {
      stdout: EventEmitter;
      stderr: EventEmitter;
      kill: () => boolean;
      killed: boolean;
      pid: number;
    };
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    child.killed = false;
    child.pid = 4321;
    child.kill = () => {
      child.killed = true;
      child.emit('exit', 0, 'SIGTERM');
      return true;
    };
    children.push({ command, args, options, child });
    return child;
  }) as any;
  const service = new GooseBridgeService({
    config: { port: 3456, tls: false, executablePath: '/tmp/goosed' },
    spawnImpl: fakeSpawn,
  });

  const status = service.start();
  assert.equal(status.running, true);
  assert.equal(children[0].command, '/tmp/goosed');
  assert.deepEqual(children[0].args, ['agent']);
  assert.equal(children[0].options.env.GOOSE_PORT, '3456');
  assert.equal(children[0].options.env.GOOSE_TLS, 'false');

  const stopped = service.stop();
  assert.equal(stopped.running, false);
});

test('service sends session reply and emits normalized SSE events', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    if (String(url).endsWith('/agent/start')) {
      return new Response('{"id":"goose-session-a"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (String(url).endsWith('/events')) {
      return new Response([
        'data: {"type":"Message","request_id":"req1","chat_request_id":"req1","message":{"content":[{"type":"text","text":"hi"}]}}\n\n',
        'data: {"type":"Finish","request_id":"req1","chat_request_id":"req1","reason":"stop"}\n\n',
      ].join(''), {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    }
    return new Response('{"request_id":"req1"}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });
  const events: any[] = [];
  service.on('event', (event) => events.push(event));

  const result = await service.sendMessage({
    sessionId: 'session-a',
    requestId: 'req1',
    text: 'hello',
  });

  assert.equal(result.eventCount, 2);
  assert.equal(calls[0].url, 'http://goose.local/agent/start');
  assert.equal(calls[1].url, 'http://goose.local/sessions/goose-session-a/events');
  assert.equal(calls[2].url, 'http://goose.local/sessions/goose-session-a/reply');
  assert.match(String(calls[2].options.body), /"request_id":"req1"/);
  assert.equal(events[0].kind, 'message');
  assert.equal(events[0].sessionId, 'session-a');
  assert.equal(events[1].kind, 'finish');
  assert.equal(service.getStatus().sessionMappings[0].gooseSessionId, 'goose-session-a');
});

test('service includes extension overrides when starting a Goose session', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const extensionOverrides = [
    {
      type: 'stdio',
      name: 'beav',
      description: 'Beav Client Tools MCP bridge',
      cmd: 'node',
      args: ['/runtime/beav-mcp/src/server.js'],
      envs: {
        BEAV_BRIDGE_URL: 'http://127.0.0.1:23456',
        BEAV_BRIDGE_PATH: '/mcp/beav',
      },
      env_keys: [],
      timeout: 120,
      bundled: true,
      available_tools: [],
    },
  ];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    if (String(url).endsWith('/agent/start')) {
      return new Response('{"id":"goose-session-a"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (String(url).endsWith('/events')) {
      return new Response('data: {"type":"Finish","request_id":"req1","chat_request_id":"req1","reason":"stop"}\n\n', {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    }
    return new Response('{"request_id":"req1"}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  await service.sendMessage({
    sessionId: 'session-a',
    requestId: 'req1',
    text: 'hello',
    config: {
      cwd: '/workspace/beav',
      extensionOverrides,
    },
  });

  assert.equal(calls[0].url, 'http://goose.local/agent/start');
  assert.deepEqual(JSON.parse(String(calls[0].options.body)), {
    working_dir: '/workspace/beav',
    extension_overrides: extensionOverrides,
  });
});

test('service can start a Goose session without sending a user reply', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const extensionOverrides = [
    {
      type: 'stdio',
      name: 'beav',
      description: 'Beav Client Tools MCP bridge',
      cmd: 'node',
      args: ['/runtime/beav-mcp/src/server.js'],
      envs: {},
      env_keys: [],
      timeout: 120,
      bundled: true,
      available_tools: [],
    },
  ];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    return new Response('{"id":"goose-session-start-only"}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  const result = await service.startSession({
    sessionId: 'app-session-start-only',
    config: {
      cwd: '/workspace/beav',
      extensionOverrides,
    },
  });

  assert.deepEqual(result, {
    appSessionId: 'app-session-start-only',
    gooseSessionId: 'goose-session-start-only',
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'http://goose.local/agent/start');
  assert.deepEqual(JSON.parse(String(calls[0].options.body)), {
    working_dir: '/workspace/beav',
    extension_overrides: extensionOverrides,
  });
  assert.equal(service.getStatus().sessionMappings[0].gooseSessionId, 'goose-session-start-only');
});

test('service updates Goose provider when provider and model are configured', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    if (String(url).endsWith('/agent/start')) {
      return new Response('{"id":"goose-session-provider"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    return new Response('{}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  await service.startSession({
    sessionId: 'app-session-provider',
    config: {
      cwd: '/workspace/beav',
      provider: 'openai',
      model: 'gpt-4o-mini',
      contextLimit: 128000,
      requestParams: {
        temperature: 0,
      },
    },
  });

  assert.equal(calls[0].url, 'http://goose.local/agent/start');
  assert.equal(calls[1].url, 'http://goose.local/agent/update_provider');
  assert.deepEqual(JSON.parse(String(calls[1].options.body)), {
    provider: 'openai',
    model: 'gpt-4o-mini',
    session_id: 'goose-session-provider',
    context_limit: 128000,
    request_params: {
      temperature: 0,
    },
  });
});

test('service cancel posts to session cancel endpoint', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  const result = await service.cancel('session-a', undefined, 'req-cancel');

  assert.equal(result.success, true);
  assert.equal(calls[0].url, 'http://goose.local/sessions/session-a/cancel');
  assert.equal(calls[0].options.method, 'POST');
  assert.equal(calls[0].options.body, '{"request_id":"req-cancel"}');
});

test('service cancel maps app session ids to Goose session ids', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    if (String(url).endsWith('/agent/start')) {
      return new Response('{"id":"goose-session-a"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (String(url).endsWith('/events')) {
      return new Response('data: {"type":"Finish","request_id":"req-map","chat_request_id":"req-map","reason":"stop"}\n\n', {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    }
    return new Response('{"request_id":"req-map"}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  await service.sendMessage({
    sessionId: 'app-session-a',
    requestId: 'req-map',
    text: 'hello',
  });
  const result = await service.cancel('app-session-a', undefined, 'req-cancel');

  const cancelCall = calls.find((call) => call.url.endsWith('/cancel'));
  assert.equal(result.success, true);
  assert.equal(cancelCall?.url, 'http://goose.local/sessions/goose-session-a/cancel');
  assert.equal(cancelCall?.options.body, '{"request_id":"req-cancel"}');
});

test('service reuses mapped Goose session for the same app session', async () => {
  const calls: Array<{ url: string; options: RequestInit }> = [];
  const fakeFetch = (async (url: string, options: RequestInit) => {
    calls.push({ url, options });
    if (String(url).endsWith('/agent/start')) {
      return new Response('{"id":"goose-session-a"}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (String(url).endsWith('/events')) {
      const body = String(options.signal ? '' : '');
      void body;
      return new Response('data: {"type":"Finish","request_id":"req","chat_request_id":"req","reason":"stop"}\n\n', {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    }
    return new Response('{"request_id":"req"}', {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as any;
  const service = new GooseBridgeService({
    config: { baseUrl: 'http://goose.local', tls: false },
    fetchImpl: fakeFetch,
  });

  await service.sendMessage({ sessionId: 'session-a', requestId: 'req-a', text: 'one' });
  await service.sendMessage({ sessionId: 'session-a', requestId: 'req-b', text: 'two' });

  assert.equal(calls.filter((call) => call.url.endsWith('/agent/start')).length, 1);
  assert.equal(calls.filter((call) => call.url.includes('/sessions/goose-session-a/reply')).length, 2);
});

test('chat adapter converts Goose message deltas to runtime events', () => {
  const adapter = new GooseChatEventAdapter({
    sessionId: 'session-a',
    requestId: 'req1',
  });

  const events = adapter.accept({
    kind: 'message',
    requestId: 'req1',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'hello' }],
    },
    raw: {},
  });

  assert.deepEqual(events.map((event) => event.eventType), ['runtime:text-delta']);
  assert.equal(events[0].sessionId, 'session-a');
  assert.equal(events[0].payload?.content, 'hello');
  assert.equal(adapter.getAssistantText(), 'hello');
});

test('chat adapter de-duplicates cumulative Goose text frames', () => {
  const adapter = new GooseChatEventAdapter({
    sessionId: 'session-a',
    requestId: 'req1',
  });

  adapter.accept({
    kind: 'message',
    requestId: 'req1',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'hello' }],
    },
    raw: {},
  });
  const events = adapter.accept({
    kind: 'message',
    requestId: 'req1',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'hello world' }],
    },
    raw: {},
  });

  assert.equal(events.length, 1);
  assert.equal(events[0].payload?.content, ' world');
  assert.equal(adapter.getAssistantText(), 'hello world');
});

test('chat adapter converts Goose tool calls and finish events', () => {
  const adapter = new GooseChatEventAdapter({
    sessionId: 'session-a',
    requestId: 'req1',
    runtimeId: 'goose:req1',
    runtimeMode: 'goose',
  });

  const toolStart = adapter.accept({
    kind: 'message',
    requestId: 'req1',
    message: {
      role: 'assistant',
      content: [{
        type: 'toolRequest',
        id: 'call-1',
        tool_call: {
          name: 'beav.open_view',
          arguments: { view: 'media-library' },
        },
      }],
    },
    raw: {},
  });
  assert.equal(toolStart[0].eventType, 'runtime:tool-start');
  assert.equal(toolStart[0].sessionId, 'session-a');
  assert.equal(toolStart[0].taskId, 'req1');
  assert.equal(toolStart[0].runtimeId, 'goose:req1');
  assert.deepEqual(toolStart[0].payload, {
    callId: 'call-1',
    name: 'beav.open_view',
    input: { view: 'media-library' },
    description: 'Goose \u8c03\u7528\u5de5\u5177\uff1abeav.open_view',
  });
  assert.equal(toolStart[0].payload?.name, 'beav.open_view');

  const toolEnd = adapter.accept({
    kind: 'message',
    requestId: 'req1',
    message: {
      role: 'assistant',
      content: [{
        type: 'toolResponse',
        tool_call_id: 'call-1',
        tool_result: { content: 'ok' },
      }],
    },
    raw: {},
  });
  assert.equal(toolEnd[0].eventType, 'runtime:tool-end');
  assert.deepEqual(toolEnd[0].payload, {
    callId: 'call-1',
    name: 'beav.open_view',
    output: { success: true, content: 'ok' },
  });

  const finish = adapter.accept({
    kind: 'finish',
    requestId: 'req1',
    reason: 'stop',
    raw: {},
  });
  assert.equal(finish[0].eventType, 'runtime:done');
  assert.deepEqual(finish[0].payload, {
    status: 'completed',
    content: '',
    runtimeMode: 'goose',
    reason: 'stop',
  });
  assert.deepEqual(adapter.finish('stop'), []);
});

test('chat adapter converts Goose errors to chat error checkpoints', () => {
  const adapter = new GooseChatEventAdapter({
    sessionId: 'session-a',
    requestId: 'req1',
  });

  const events = adapter.accept({
    kind: 'error',
    requestId: 'req1',
    error: 'boom',
    raw: {},
  });

  assert.equal(events[0].eventType, 'runtime:checkpoint');
  assert.equal(events[0].payload?.checkpointType, 'chat.error');
  assert.deepEqual((events[0].payload?.payload as Record<string, unknown>).raw, 'boom');
});
