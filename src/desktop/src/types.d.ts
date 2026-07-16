export { };
// Type definitions
export interface VideoEntry {
  id: string;
  title: string;
  publishedAt: string;
  status: 'pending' | 'downloading' | 'success' | 'failed';
  retryCount: number;
  errorMessage?: string;
  subtitleFile?: string;
}

export interface ToolDiagnosticDescriptor {
  name: string;
  displayName: string;
  description: string;
  kind: string;
  visibility: 'public' | 'developer' | 'internal';
  contexts: string[];
  availabilityStatus: 'available' | 'missing_context' | 'internal_only' | 'not_in_current_pack' | 'registration_error';
  availabilityReason: string;
}

export interface ToolDiagnosticRunResult {
  success: boolean;
  mode: 'direct' | 'ai';
  toolName: string;
  request: unknown;
  response?: unknown;
  error?: string;
  toolCallReturned?: boolean;
  toolNameMatched?: boolean;
  argumentsParsed?: boolean;
  executionSucceeded?: boolean;
}

export interface NotificationSettingsPayload {
  enabled: boolean;
  inApp: {
    enabled: boolean;
    maxVisible: number;
    autoCloseMs: number;
  };
  sound: {
    enabled: boolean;
    volume: number;
    muteWhenFocused: boolean;
    success: boolean;
    failure: boolean;
    attention: boolean;
  };
  system: {
    enabled: boolean;
  };
  quietHours: {
    enabled: boolean;
    start: string;
    end: string;
  };
  rules: {
    runtimeBackgroundDone: boolean;
    runtimeFailed: boolean;
    runtimeNeedsApproval: boolean;
    generationCompleted: boolean;
    generationFailed: boolean;
    redclawCompleted: boolean;
    redclawFailed: boolean;
  };
}

export interface NotificationPermissionState {
  state: 'granted' | 'denied' | 'prompt' | 'unknown';
}

export interface AgentTaskNode {
  id: string;
  type: string;
  title: string;
  status: 'pending' | 'running' | 'completed' | 'failed' | 'skipped';
  startedAt?: number;
  completedAt?: number;
  summary?: string;
  error?: string;
}

export interface AgentTaskCheckpoint {
  id: string;
  nodeId: string;
  summary: string;
  payload?: unknown;
  createdAt: number;
}

export interface AgentTaskArtifact {
  id: string;
  type: string;
  label: string;
  path?: string;
  metadata?: unknown;
  createdAt: number;
}

export interface IntentRouteInfo {
  intent: string;
  goal: string;
  requiredCapabilities: string[];
  recommendedRole: string;
  requiresLongRunningTask: boolean;
  requiresMultiAgent: boolean;
  requiresHumanApproval: boolean;
  confidence: number;
  reasoning: string;
}

export interface AgentTaskSnapshot {
  id: string;
  taskType: string;
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';
  runtimeMode: string;
  ownerSessionId?: string | null;
  intent?: string | null;
  roleId?: string | null;
  goal?: string | null;
  currentNode?: string | null;
  route?: IntentRouteInfo | null;
  graph: AgentTaskNode[];
  artifacts: AgentTaskArtifact[];
  checkpoints: AgentTaskCheckpoint[];
  metadata?: unknown;
  lastError?: string | null;
  createdAt: number;
  updatedAt: number;
  startedAt?: number | null;
  completedAt?: number | null;
}

export interface AgentTaskTrace {
  id: number;
  taskId: string;
  nodeId?: string | null;
  runtimeId?: string | null;
  parentRuntimeId?: string | null;
  sourceTaskId?: string | null;
  eventType: string;
  payload?: unknown;
  createdAt: number;
}

export type RuntimeUnifiedEventType =
  | 'runtime:stream-start'
  | 'runtime:text-delta'
  | 'runtime:done'
  | 'runtime:tool-start'
  | 'runtime:tool-update'
  | 'runtime:tool-end'
  | 'runtime:task-node-changed'
  | 'runtime:subagent-started'
  | 'runtime:subagent-finished'
  | 'runtime:checkpoint'
  | 'runtime:cli-tool-detected'
  | 'runtime:cli-install-started'
  | 'runtime:cli-install-finished'
  | 'runtime:cli-execution-started'
  | 'runtime:cli-execution-log'
  | 'runtime:cli-execution-status'
  | 'runtime:cli-escalation-requested'
  | 'runtime:cli-escalation-resolved'
  | 'runtime:cli-verification-finished'
  | 'stream_start'
  | 'text_delta'
  | 'tool_request'
  | 'tool_result'
  | 'task_node_changed'
  | 'subagent_spawned'
  | 'subagent_finished'
  | 'task_checkpoint_saved';

export interface RuntimeUnifiedEvent {
  eventType: RuntimeUnifiedEventType;
  sessionId?: string | null;
  taskId?: string | null;
  runtimeId?: string | null;
  parentRuntimeId?: string | null;
  payload?: unknown;
  timestamp: number;
}

export type CliRuntimeToolSource =
  | 'system'
  | 'app-managed'
  | 'workspace-managed'
  | 'user-declared'
  | 'unknown';

export type CliRuntimeToolHealth =
  | 'unknown'
  | 'ready'
  | 'missing'
  | 'broken';

export type CliRuntimeResolvedFrom =
  | 'host-shell-path'
  | 'extra-bin-path'
  | 'managed-environment'
  | 'explicit-path'
  | 'unknown';

export type CliRuntimeEnvironmentScope =
  | 'app-global'
  | 'workspace-local'
  | 'task-ephemeral';

export type CliRuntimeExecutionStatus =
  | 'pending'
  | 'running'
  | 'waiting-approval'
  | 'completed'
  | 'failed'
  | 'cancelled';

export type CliRuntimeEscalationScope = 'once' | 'session' | 'always';

export interface CliRuntimeToolRecord {
  id: string;
  name: string;
  executable: string;
  resolvedPath?: string | null;
  resolvedFrom?: CliRuntimeResolvedFrom | null;
  source: CliRuntimeToolSource;
  installMethod?: string | null;
  installSpec?: string | null;
  version?: string | null;
  health: CliRuntimeToolHealth;
  manifestId?: string | null;
  environmentId?: string | null;
  lastCheckedAt?: number | null;
  effectivePathPreview?: string[];
  searchedPathEntriesCount?: number | null;
  isInDefaultDetectCatalog?: boolean;
  metadata?: Record<string, unknown> | null;
}

export interface CliRuntimeEnvironmentRecord {
  id: string;
  scope: CliRuntimeEnvironmentScope;
  rootPath: string;
  workspaceRoot?: string | null;
  pathEntries: string[];
  installedToolIds: string[];
  runtimes?: Record<string, unknown> | null;
  createdAt?: number | null;
  updatedAt?: number | null;
  metadata?: Record<string, unknown> | null;
}

export interface CliRuntimeVerificationRecord {
  ruleType?: string;
  status?: 'passed' | 'failed' | 'skipped' | 'unknown';
  summary?: string;
  detail?: string;
  payload?: Record<string, unknown> | null;
}

export interface CliRuntimeExecutionRecord {
  id: string;
  sessionId?: string | null;
  taskId?: string | null;
  runtimeId?: string | null;
  environmentId?: string | null;
  toolId?: string | null;
  toolName?: string | null;
  argv: string[];
  cwd?: string | null;
  commandPreview?: string | null;
  status: CliRuntimeExecutionStatus;
  usePty?: boolean;
  exitCode?: number | null;
  summary?: string | null;
  lastLogChunk?: string | null;
  startedAt?: number | null;
  updatedAt?: number | null;
  completedAt?: number | null;
  verificationResults?: CliRuntimeVerificationRecord[];
  metadata?: Record<string, unknown> | null;
}

export interface CliRuntimeEscalationRequest {
  escalationId: string;
  sessionId?: string | null;
  taskId?: string | null;
  runtimeId?: string | null;
  executionId?: string | null;
  title: string;
  description: string;
  reason?: string;
  commandPreview?: string;
  permissionSummary?: string[];
  scopeOptions?: CliRuntimeEscalationScope[];
  requestedAt?: number | null;
  metadata?: Record<string, unknown> | null;
}

export interface SessionRuntimeRecord {
  id: number;
  sessionId: string;
  recordType: string;
  role: string;
  content: string;
  payload?: unknown;
  createdAt: number;
}

export interface DiagnosticsLogStatus {
  enabled: boolean;
  logDirectory: string;
  reportDirectory?: string;
  retentionDays?: number;
  maxFileMb?: number;
  recentPreviewLimit?: number;
  uploadConfigured?: boolean;
  uploadEndpoint?: string | null;
  pendingCount?: number;
  debugVerboseEnabled?: boolean;
  previousUncleanShutdown?: boolean;
}

export interface DiagnosticsPendingReport {
  id: string;
  trigger: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  summary: string;
  includeAdvancedContext: boolean;
  lastError?: string | null;
  uploadedAt?: string | null;
  lastAttemptAt?: string | null;
  dedupeKey?: string | null;
  bundleFileName?: string | null;
  metadata?: unknown;
}

export interface SessionCheckpointRecord {
  id: string;
  sessionId: string;
  runtimeId?: string | null;
  parentRuntimeId?: string | null;
  sourceTaskId?: string | null;
  checkpointType: string;
  summary: string;
  payload?: unknown;
  createdAt: number;
}

export interface SessionToolResultItem {
  id: string;
  sessionId: string;
  runtimeId?: string | null;
  parentRuntimeId?: string | null;
  sourceTaskId?: string | null;
  callId: string;
  toolName: string;
  command?: string;
  success: boolean;
  resultText?: string;
  summaryText?: string;
  promptText?: string;
  originalChars?: number;
  promptChars?: number;
  truncated: boolean;
  payload?: unknown;
  createdAt: number;
  updatedAt: number;
}

export interface SessionBridgeSessionSummary {
  id: string;
  title: string;
  updatedAt: number;
  createdAt: number;
  contextType: string;
  runtimeMode: string;
  isBackgroundSession: boolean;
  ownerTaskCount: number;
  backgroundTaskCount: number;
}

export interface SessionBridgeStatus {
  enabled: boolean;
  listening: boolean;
  host: string;
  port: number;
  authToken: string;
  websocketUrl: string;
  httpBaseUrl: string;
  subscriberCount: number;
  lastError: string | null;
}

export interface SessionBridgeSnapshot {
  session: SessionBridgeSessionSummary & {
    metadata?: Record<string, unknown>;
  };
  transcript: SessionRuntimeRecord[];
  checkpoints: SessionCheckpointRecord[];
  toolResults: SessionToolResultItem[];
  tasks: AgentTaskSnapshot[];
  backgroundTasks: Array<{
    id: string;
    kind: string;
    title: string;
    status: string;
    phase: string;
    sessionId?: string;
    contextId?: string;
    error?: string;
    summary?: string;
    latestText?: string;
    attemptCount: number;
    workerState: string;
    workerMode?: string;
    workerPid?: number;
    workerLabel?: string;
    workerLastHeartbeatAt?: string;
    cancelReason?: string;
    rollbackState: string;
    rollbackError?: string;
    createdAt: string;
    updatedAt: string;
    completedAt?: string;
    turns: Array<{
      id: string;
      at: string;
      text: string;
      source: 'thought' | 'tool' | 'response' | 'system';
    }>;
  }>;
  permissionRequests: SessionBridgePermissionRequest[];
}

export interface SessionBridgePermissionRequest {
  id: string;
  sessionId: string;
  callId: string;
  toolName: string;
  params: Record<string, unknown>;
  details: {
    type: 'edit' | 'exec' | 'info';
    title: string;
    description: string;
    impact?: string;
  };
  createdAt: number;
  resolvedAt?: number;
  status: 'pending' | 'approved_once' | 'approved_always' | 'cancelled';
  decision?: 'proceed_once' | 'proceed_always' | 'cancel';
}

export interface IpcInvokeGuardOptions<T = unknown> {
  timeoutMs?: number;
  fallback?: T | null | (() => T | null);
  normalize?: (value: unknown) => T;
}

export interface FileIndexLaneStatus {
  lane: string;
  label: string;
  status: string;
  done: number;
  total: number;
  failed: number;
  metadataOnly: number;
  lastUpdatedAt: string | null;
  nextRetryAt: string | null;
}

export interface FileIndexScopeStatus {
  scopeId: string;
  name: string;
  scopeType: string;
  ownerId: string;
  ownerName: string;
  fileCount: number;
  status: string;
  failedCount: number;
  lanes: FileIndexLaneStatus[];
}

export interface FileIndexDashboard {
  overall: {
    status: string;
    indexedFiles: number;
    totalFiles: number;
    failedFiles: number;
    lastIndexedAt: string | null;
  };
  lanes: FileIndexLaneStatus[];
  scopes: FileIndexScopeStatus[];
}

export interface RoleSpec {
  roleId: string;
  purpose: string;
  systemPrompt: string;
  allowedToolPack: string;
  inputSchema: string;
  outputSchema: string;
  handoffContract: string;
  artifactTypes: string[];
}

declare global {
  interface ChatSession {
    id: string;
    title: string;
    updatedAt: string;
  }

  interface ContextChatSessionListItem {
    id: string;
    messageCount: number;
    summary: string;
    transcriptCount: number;
    checkpointCount: number;
    context?: unknown;
    chatSession?: {
      id: string;
      title?: string;
      updatedAt?: string;
    } | null;
  }

  interface ChatMessage {
    id: string;
    session_id: string;
    role: string;
    content: string;
    tool_call_id?: string;
    created_at: string;
  }

  interface SubjectCategory {
    id: string;
    name: string;
    createdAt: string;
    updatedAt: string;
  }

  interface SubjectAttribute {
    key: string;
    value: string;
  }

  interface SubjectRecord {
    id: string;
    name: string;
    categoryId?: string;
    description?: string;
    tags: string[];
    attributes: SubjectAttribute[];
    imagePaths: string[];
    voicePath?: string;
    voiceScript?: string;
    createdAt: string;
    updatedAt: string;
    absoluteImagePaths?: string[];
    previewUrls?: string[];
    primaryPreviewUrl?: string;
    absoluteVoicePath?: string;
    voicePreviewUrl?: string;
  }

  interface SkillDefinition {
    name: string;
    description: string;
    location: string;
    body: string;
    baseDir?: string;
    aliases?: string[];
    sourceScope?: string;
    isBuiltin?: boolean;
    disabled?: boolean;
  }

  interface ToolConfirmationDetails {
    type: 'edit' | 'exec' | 'info';
    title: string;
    description: string;
    impact?: string;
  }

  interface ToolConfirmRequest {
    callId: string;
    name: string;
    details: ToolConfirmationDetails;
  }
}
