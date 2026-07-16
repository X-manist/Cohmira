export type GooseBridgeProtocol = 'http' | 'https';

export interface GooseBridgeMcpMount {
  id: string;
  name?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string | undefined>;
  url?: string;
  enabled?: boolean;
}

export interface GooseBridgeLongTermMemoryConfig {
  enabled?: boolean;
  mountPath?: string;
  provider?: string;
  config?: Record<string, unknown>;
}

export interface GooseBridgeConfig {
  host?: string;
  port?: number;
  tls?: boolean;
  secretKey?: string;
  executablePath?: string;
  commandArgs?: string[];
  cwd?: string;
  env?: Record<string, string | undefined>;
  endpointPrefix?: string;
  baseUrl?: string;
  useSessionEvents?: boolean;
  gooseHome?: string;
  tlsCertPath?: string;
  tlsKeyPath?: string;
  addBinaryDirToPath?: boolean;
  mcp?: GooseBridgeMcpMount[];
  longTermMemory?: GooseBridgeLongTermMemoryConfig;
  extensionOverrides?: unknown[];
  provider?: string;
  model?: string;
  contextLimit?: number;
  requestParams?: Record<string, unknown>;
}

export interface GooseSessionMapping {
  appSessionId: string;
  gooseSessionId: string;
  createdAt: number;
  updatedAt: number;
}

export interface GooseSidecarCommand {
  command: string;
  args: string[];
  env: Record<string, string>;
  cwd?: string;
}

export interface GooseEndpointSet {
  baseUrl: string;
  statusUrl: string;
  startAgentUrl: string;
  resumeAgentUrl: string;
  toolsUrl: string;
  configUrl: string;
}

export interface GooseOpenAiEndpointConfig {
  host?: string;
  basePath?: string;
  baseUrl?: string;
}

export type GooseReplyEndpoint =
  | {
      kind: 'session';
      method: 'POST';
      replyUrl: string;
      eventsUrl: string;
      cancelUrl: string;
      sessionId: string;
    }
  | {
      kind: 'legacy';
      method: 'POST';
      replyUrl: string;
    };

export type GooseBridgeTaskStatus =
  | 'queued'
  | 'running'
  | 'canceling'
  | 'cancelled'
  | 'completed'
  | 'failed';

export interface GooseBridgeTaskSnapshot {
  id: string;
  status: GooseBridgeTaskStatus;
  enqueuedAt: number;
  startedAt?: number;
  finishedAt?: number;
  error?: string;
}

export type GooseToolCallStatus =
  | 'requested'
  | 'waiting_permission'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface GooseToolCallState {
  id: string;
  requestId?: string;
  sessionId?: string;
  name: string;
  arguments: Record<string, unknown>;
  status: GooseToolCallStatus;
  prompt?: string;
  result?: unknown;
  error?: string;
  updatedAt: number;
}

export interface GooseBridgePermissionRequest {
  id: string;
  requestId?: string;
  sessionId?: string;
  toolCallId?: string;
  toolName: string;
  arguments: Record<string, unknown>;
  prompt?: string;
  status: 'pending' | 'approved_once' | 'approved_always' | 'denied' | 'cancelled';
  createdAt: number;
  resolvedAt?: number;
}

export interface GooseTokenState {
  input_tokens?: number;
  output_tokens?: number;
  total_tokens?: number;
  accumulated_input_tokens?: number;
  accumulated_output_tokens?: number;
  accumulated_total_tokens?: number;
  accumulated_cost?: number | null;
  [key: string]: unknown;
}

export interface GooseMessage {
  id?: string | null;
  role?: string;
  created?: number;
  content?: Array<Record<string, unknown>>;
  metadata?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface GooseSseFrame {
  id?: string;
  event?: string;
  data?: string;
  retry?: number;
  comment?: string;
}

export type GooseNormalizedEvent =
  | {
      kind: 'message';
      requestId?: string;
      chatRequestId?: string;
      message: GooseMessage;
      tokenState?: GooseTokenState;
      raw: Record<string, unknown>;
    }
  | {
      kind: 'error';
      requestId?: string;
      chatRequestId?: string;
      error: string;
      raw?: Record<string, unknown>;
    }
  | {
      kind: 'finish';
      requestId?: string;
      chatRequestId?: string;
      reason: string;
      tokenState?: GooseTokenState;
      raw: Record<string, unknown>;
    }
  | {
      kind: 'notification';
      requestId?: string;
      chatRequestId?: string;
      notificationRequestId?: string;
      message: unknown;
      raw: Record<string, unknown>;
    }
  | {
      kind: 'conversation';
      requestId?: string;
      chatRequestId?: string;
      conversation: unknown;
      raw: Record<string, unknown>;
    }
  | {
      kind: 'active_requests';
      requestIds: string[];
      raw: Record<string, unknown>;
    }
  | {
      kind: 'ping';
      id?: string;
      raw?: Record<string, unknown>;
    }
  | {
      kind: 'unknown';
      requestId?: string;
      chatRequestId?: string;
      type?: string;
      raw: Record<string, unknown>;
    };

export interface GooseReplyRequestInput {
  sessionId: string;
  requestId: string;
  message: GooseMessage;
  overrideConversation?: GooseMessage[];
}
