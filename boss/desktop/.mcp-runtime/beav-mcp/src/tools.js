'use strict';

const { BeavBridgeClient, bridgeErrorPayload, buildBridgeConfig } = require('./beavBridge');
const { toolContent } = require('./protocol');

const SERVER_VERSION = '0.1.0';

const BEAV_VIEWS = [
  { id: 'chat', title: 'Chat', description: 'Beav chat workspace.' },
  { id: 'team', title: 'Team', description: 'Advisor/team chat workspace.' },
  { id: 'skills', title: 'Skills', description: 'Skill management.' },
  { id: 'knowledge', title: 'Knowledge', description: 'Knowledge library and search.' },
  { id: 'settings', title: 'Settings', description: 'Application settings.' },
  { id: 'manuscripts', title: 'Manuscripts', description: 'Manuscript and content workspace.' },
  { id: 'archives', title: 'Archives', description: 'Archive profiles and samples.' },
  { id: 'wander', title: 'Wander', description: 'Random references and brainstorm flows.' },
  { id: 'redclaw', title: 'RedClaw', description: 'RedClaw content project automation.' },
  { id: 'media-library', title: 'Media Library', description: 'Imported and generated media assets.' },
  { id: 'cover-studio', title: 'Cover Studio', description: 'Cover template and image workflow.' },
  { id: 'generation-studio', title: 'Generation Studio', description: 'Image/video generation workspace.' },
  { id: 'subjects', title: 'Subjects', description: 'Subject, persona, and product library.' },
  { id: 'workboard', title: 'Workboard', description: 'Unified work items and automation board.' },
];

const BRIDGE_ACTIONS = ['app_cli', 'navigate', 'tool_call'];

const READ_ONLY_COMMANDS = new Map([
  ['help', new Set(['show'])],
  ['workspace', new Set(['list', 'show', 'get'])],
  ['work', new Set(['list', 'get', 'ready'])],
  ['spaces', new Set(['list'])],
  ['manuscripts', new Set(['list', 'read', 'clips'])],
  ['knowledge', new Set(['list', 'get', 'search'])],
  ['advisors', new Set(['list', 'get', 'search'])],
  ['memory', new Set(['list', 'get', 'search'])],
  ['redclaw', new Set(['list', 'get', 'runner-status'])],
  ['media', new Set(['list', 'get'])],
  ['subjects', new Set(['list', 'get', 'search', 'categories'])],
  ['mcp', new Set(['list', 'tools', 'test', 'status', 'oauth-status'])],
  ['settings', new Set(['get', 'show'])],
  ['skills', new Set(['list', 'get'])],
  ['archives', new Set(['profiles', 'samples', 'list', 'get'])],
  ['wander', new Set(['list', 'get', 'random'])],
]);

function tokenize(input) {
  const tokens = [];
  const regex = /"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|(\S+)/g;
  let match;
  while ((match = regex.exec(String(input || ''))) !== null) {
    if (match[1] !== undefined) tokens.push(match[1].replace(/\\"/g, '"'));
    else if (match[2] !== undefined) tokens.push(match[2].replace(/\\'/g, "'"));
    else tokens.push(match[3]);
  }
  return tokens;
}

function parseCommand(command) {
  const tokens = tokenize(command);
  while (tokens.length > 0 && ['app-cli', 'app_cli', 'redconvert', 'redconvert-cli'].includes(tokens[0].toLowerCase())) {
    tokens.shift();
  }
  if (tokens.length === 0) return { namespace: 'help', action: 'show' };
  const namespace = String(tokens.shift() || 'help').toLowerCase();
  const action = tokens[0] && !tokens[0].startsWith('--') ? String(tokens.shift()).toLowerCase() : 'list';
  return { namespace, action };
}

function isReadOnlyCommand(command) {
  const parsed = parseCommand(command);
  const actions = READ_ONLY_COMMANDS.get(parsed.namespace);
  return Boolean(actions && actions.has(parsed.action));
}

function requireObject(value, fieldName) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${fieldName} must be an object`);
  }
  return value;
}

function requireString(value, fieldName) {
  const text = String(value || '').trim();
  if (!text) throw new Error(`${fieldName} is required`);
  return text;
}

function normalizeBool(value, defaultValue = false) {
  if (value === undefined || value === null) return defaultValue;
  if (typeof value === 'boolean') return value;
  return ['1', 'true', 'yes', 'on'].includes(String(value).trim().toLowerCase());
}

function safePath(value) {
  const text = requireString(value, 'path').replace(/\\/g, '/').replace(/^\.\/+/, '');
  const segments = text.split('/');
  if (
    !text ||
    text === '.' ||
    text === '..' ||
    text.startsWith('/') ||
    /^[A-Za-z]:\//.test(text) ||
    text.includes('\0') ||
    segments.some((segment) => segment === '.' || segment === '..')
  ) {
    throw new Error('path must be workspace-relative and cannot traverse outside the Beav workspace');
  }
  return text;
}

function resolveBridgeConfigForDisplay(config = {}) {
  const defaults = buildBridgeConfig();
  return {
    ...defaults,
    ...config,
  };
}

function endpointFromConfig(config) {
  const baseUrl = String(config.baseUrl || '').replace(/\/+$/g, '');
  const path = String(config.path || '/mcp/beav');
  return `${baseUrl}${path.startsWith('/') ? path : `/${path}`}`;
}

function isErrorPayload(value) {
  return Boolean(
    value &&
    typeof value === 'object' &&
    (
      value.ok === false ||
      value.success === false ||
      value.blocked === true ||
      Object.prototype.hasOwnProperty.call(value, 'error')
    )
  );
}

function makeTool(name, description, inputSchema, handler) {
  return { name, description, inputSchema, handler };
}

function createTools(options = {}) {
  const bridgeFactory = options.bridgeFactory || (() => new BeavBridgeClient(options.bridgeConfig));

  const tools = [
    makeTool(
      'list_capabilities',
      'List Beav MCP capabilities and local bridge configuration. This does not require the Beav bridge to be running.',
      {
        type: 'object',
        additionalProperties: false,
        properties: {
          includeBridgeConfig: {
            type: 'boolean',
            description: 'Include the local bridge endpoint/path expected by this MCP server.',
            default: true,
          },
        },
        required: [],
      },
      async (args) => {
        const includeBridgeConfig = normalizeBool(args.includeBridgeConfig, true);
        const bridgeConfig = resolveBridgeConfigForDisplay(options.bridgeConfig);
        return {
          ok: true,
          server: {
            name: 'beav-mcp',
            version: SERVER_VERSION,
            protocol: 'stdio',
          },
          capabilities: [
            'list Beav views',
            'navigate/open Beav views through a local bridge',
            'list Beav workspaces/spaces through app_cli',
            'create or update manuscript notes through app_cli',
            'run Beav app_cli commands with dry-run and confirmation guards',
            'passthrough Beav internal tool calls with confirmation guards',
          ],
          tools: listToolDefinitions(tools).map((tool) => ({
            name: tool.name,
            description: tool.description,
          })),
          views: BEAV_VIEWS.map((view) => ({
            id: view.id,
            title: view.title,
          })),
          safety: {
            writeToolsDefaultToDryRun: true,
            destructiveOrPassthroughCallsRequireConfirm: true,
          },
          bridge: includeBridgeConfig
            ? {
                url: bridgeConfig.baseUrl,
                path: bridgeConfig.path,
                endpoint: endpointFromConfig(bridgeConfig),
                actions: BRIDGE_ACTIONS,
                env: {
                  url: ['BEAV_BRIDGE_URL', 'BEAV_LOCAL_BRIDGE_URL'],
                  path: 'BEAV_BRIDGE_PATH',
                  token: ['BEAV_BRIDGE_TOKEN', 'BEAV_LOCAL_BRIDGE_TOKEN'],
                  timeoutMs: 'BEAV_BRIDGE_TIMEOUT_MS',
                },
                status: 'not_checked',
              }
            : undefined,
        };
      }
    ),
    makeTool(
      'list_views',
      'List known Beav desktop/workbench views that can be opened by open_view.',
      {
        type: 'object',
        additionalProperties: false,
        properties: {},
        required: [],
      },
      async () => ({
        ok: true,
        count: BEAV_VIEWS.length,
        views: BEAV_VIEWS,
      })
    ),
    makeTool(
      'list_workspaces',
      'List Beav workspaces/spaces via the Beav local bridge and app_cli spaces list.',
      {
        type: 'object',
        additionalProperties: false,
        properties: {},
        required: [],
      },
      async () => bridgeFactory().listWorkspaces()
    ),
    makeTool(
      'open_view',
      'Navigate the Beav desktop to a specific view through the local bridge.',
      {
        type: 'object',
        additionalProperties: false,
        required: ['view'],
        properties: {
          view: {
            type: 'string',
            enum: BEAV_VIEWS.map((view) => view.id),
            description: 'Beav view id to open.',
          },
          params: {
            type: 'object',
            description: 'Optional view-specific parameters, such as manuscript path or project id.',
            additionalProperties: true,
          },
          focusWindow: {
            type: 'boolean',
            description: 'Ask Beav to focus/raise its desktop window.',
            default: true,
          },
        },
      },
      async (args) => {
        const view = requireString(args.view, 'view');
        if (!BEAV_VIEWS.some((item) => item.id === view)) {
          throw new Error(`Unsupported Beav view: ${view}`);
        }
        return bridgeFactory().navigate({
          view,
          params: args.params && typeof args.params === 'object' ? args.params : {},
          focusWindow: normalizeBool(args.focusWindow, true),
        });
      }
    ),
    makeTool(
      'create_note',
      'Create or update a Beav manuscript note. Defaults to dryRun; set dryRun=false and confirm=true for real writes.',
      {
        type: 'object',
        additionalProperties: false,
        required: ['path', 'content'],
        properties: {
          path: {
            type: 'string',
            description: 'Workspace-relative manuscript path, for example drafts/idea.md.',
          },
          content: {
            type: 'string',
            description: 'Markdown content to write.',
          },
          metadata: {
            type: 'object',
            description: 'Frontmatter/manifest metadata to merge into the note.',
            additionalProperties: true,
          },
          dryRun: {
            type: 'boolean',
            description: 'When true, only return the planned Beav app_cli command and payload.',
            default: true,
          },
          confirm: {
            type: 'boolean',
            description: 'Must be true when dryRun=false.',
            default: false,
          },
        },
      },
      async (args) => {
        const path = safePath(args.path);
        const content = String(args.content || '');
        const metadata = args.metadata && typeof args.metadata === 'object' && !Array.isArray(args.metadata) ? args.metadata : {};
        const dryRun = normalizeBool(args.dryRun, true);
        const plan = {
          command: `manuscripts write --path ${JSON.stringify(path)}`,
          payload: {
            content,
            metadata,
          },
        };
        if (dryRun) {
          return {
            ok: true,
            dryRun: true,
            plannedAction: plan,
          };
        }
        if (!normalizeBool(args.confirm, false)) {
          return {
            ok: false,
            blocked: true,
            error: {
              code: 'CONFIRMATION_REQUIRED',
              message: 'create_note requires confirm=true when dryRun=false.',
            },
            plannedAction: plan,
          };
        }
        return bridgeFactory().createNote({ path, content, metadata });
      }
    ),
    makeTool(
      'run_app_command',
      'Run Beav app_cli command through the local bridge. Read-only commands may run directly; mutating commands require dryRun=false and confirm=true.',
      {
        type: 'object',
        additionalProperties: false,
        required: ['command'],
        properties: {
          command: {
            type: 'string',
            description: 'Beav app_cli command, for example "spaces list" or "manuscripts list".',
          },
          payload: {
            type: 'object',
            description: 'Optional structured payload for complex Beav app_cli commands.',
            additionalProperties: true,
          },
          dryRun: {
            type: 'boolean',
            description: 'When true, return the planned command instead of calling Beav.',
          },
          confirm: {
            type: 'boolean',
            description: 'Must be true for mutating or execute-like commands.',
            default: false,
          },
        },
      },
      async (args) => {
        const command = requireString(args.command, 'command');
        const payload = args.payload === undefined ? {} : requireObject(args.payload, 'payload');
        const readOnly = isReadOnlyCommand(command);
        const dryRun = args.dryRun === undefined ? !readOnly : normalizeBool(args.dryRun, false);
        const plan = { command, payload, readOnly };

        if (dryRun) {
          return {
            ok: true,
            dryRun: true,
            plannedAction: plan,
          };
        }
        if (!readOnly && !normalizeBool(args.confirm, false)) {
          return {
            ok: false,
            blocked: true,
            error: {
              code: 'CONFIRMATION_REQUIRED',
              message: 'Mutating Beav app_cli commands require confirm=true when dryRun=false.',
            },
            plannedAction: plan,
          };
        }
        return bridgeFactory().runAppCommand(command, payload);
      }
    ),
    makeTool(
      'call_beav_tool',
      'Passthrough call to a Beav internal tool exposed by the local bridge. Defaults to dryRun and requires confirm=true for real calls.',
      {
        type: 'object',
        additionalProperties: false,
        required: ['name'],
        properties: {
          name: {
            type: 'string',
            description: 'Internal Beav tool name.',
          },
          arguments: {
            type: 'object',
            description: 'Tool arguments passed through to Beav.',
            additionalProperties: true,
          },
          dryRun: {
            type: 'boolean',
            description: 'When true, return the planned tool call instead of calling Beav.',
            default: true,
          },
          confirm: {
            type: 'boolean',
            description: 'Must be true when dryRun=false.',
            default: false,
          },
        },
      },
      async (args) => {
        const name = requireString(args.name, 'name');
        const toolArgs = args.arguments === undefined ? {} : requireObject(args.arguments, 'arguments');
        const dryRun = normalizeBool(args.dryRun, true);
        const plan = { name, arguments: toolArgs };
        if (dryRun) {
          return {
            ok: true,
            dryRun: true,
            plannedAction: plan,
          };
        }
        if (!normalizeBool(args.confirm, false)) {
          return {
            ok: false,
            blocked: true,
            error: {
              code: 'CONFIRMATION_REQUIRED',
              message: 'call_beav_tool requires confirm=true when dryRun=false.',
            },
            plannedAction: plan,
          };
        }
        return bridgeFactory().callTool(name, toolArgs);
      }
    ),
  ];

  return new Map(tools.map((tool) => [tool.name, tool]));
}

function listToolDefinitions(tools) {
  return Array.from(tools.values()).map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: tool.inputSchema,
  }));
}

async function callTool(tools, name, args = {}) {
  const tool = tools.get(name);
  if (!tool) {
    return toolContent({
      ok: false,
      error: {
        code: 'UNKNOWN_TOOL',
        message: `Unknown Beav MCP tool: ${name}`,
      },
    }, true);
  }

  try {
    const result = await tool.handler(args && typeof args === 'object' && !Array.isArray(args) ? args : {});
    return toolContent(result, isErrorPayload(result));
  } catch (error) {
    return toolContent(bridgeErrorPayload(error, { tool: name }), true);
  }
}

module.exports = {
  SERVER_VERSION,
  BEAV_VIEWS,
  BRIDGE_ACTIONS,
  READ_ONLY_COMMANDS,
  createTools,
  listToolDefinitions,
  callTool,
  isReadOnlyCommand,
  parseCommand,
};
