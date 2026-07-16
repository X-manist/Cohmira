"use strict";

const { spawn } = require("node:child_process");
const path = require("node:path");

const DEFAULT_SAU_BIN = "sau";
const SECRET_KEY_PATTERN = /(password|passwd|token|secret|cookie|authorization|auth|credential)/i;

const PLATFORMS = Object.freeze({
  douyin: {
    label: "Douyin",
    cli: "douyin",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: true,
      schedule: true,
      headless: true,
      thumbnails: ["thumbnail", "thumbnailLandscape", "thumbnailPortrait"],
      maxNoteImages: 35,
      disallowGifNoteImages: true,
    },
    extraVideoOptions: ["productLink", "productTitle"],
  },
  kuaishou: {
    label: "Kuaishou",
    cli: "kuaishou",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: true,
      schedule: true,
      headless: true,
      thumbnails: ["thumbnail"],
    },
    extraVideoOptions: [],
  },
  xiaohongshu: {
    label: "Xiaohongshu",
    cli: "xiaohongshu",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: true,
      schedule: true,
      headless: true,
      thumbnails: ["thumbnail"],
      maxTags: 10,
    },
    extraVideoOptions: [],
  },
  bilibili: {
    label: "Bilibili",
    cli: "bilibili",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: false,
      schedule: true,
      headless: false,
      thumbnails: [],
      requiresTid: true,
    },
    extraVideoOptions: ["tid"],
  },
  tencent: {
    label: "Tencent/WeChat Channels",
    cli: "tencent",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: false,
      schedule: true,
      headless: true,
      thumbnails: ["thumbnail", "thumbnailLandscape", "thumbnailPortrait"],
    },
    extraVideoOptions: ["shortTitle", "category", "draft"],
  },
  youtube: {
    label: "YouTube",
    cli: "youtube",
    supports: {
      check: true,
      login: true,
      uploadVideo: true,
      uploadNote: false,
      schedule: false,
      headless: true,
      thumbnails: ["thumbnail"],
    },
    extraVideoOptions: ["playlist", "visibility"],
  },
});

const MCP_ERROR_CODES = Object.freeze({
  invalidParams: "INVALID_PARAMS",
  unsupportedPlatform: "UNSUPPORTED_PLATFORM",
  unsupportedOperation: "UNSUPPORTED_OPERATION",
  publishConfirmationRequired: "PUBLISH_CONFIRMATION_REQUIRED",
  commandFailed: "COMMAND_FAILED",
});

const COMMON_ARG_FIELDS = Object.freeze(["platform", "operation", "action", "account", "sauBin", "sau_bin"]);
const EXECUTION_ARG_FIELDS = Object.freeze(["dryRun", "confirm", "timeoutMs"]);
const RUNTIME_ARG_FIELDS = Object.freeze(["headless", "headed", "debug"]);
const UPLOAD_VIDEO_ARG_FIELDS = Object.freeze([
  "file",
  "title",
  "desc",
  "tags",
  "schedule",
  "thumbnail",
  "thumbnailLandscape",
  "thumbnailPortrait",
  "productLink",
  "productTitle",
  "tid",
  "shortTitle",
  "category",
  "draft",
  "playlist",
  "visibility",
]);
const UPLOAD_NOTE_ARG_FIELDS = Object.freeze(["images", "title", "note", "tags", "schedule", "bgm"]);
const ALLOWED_FIELDS_BY_OPERATION = Object.freeze({
  check: new Set([...COMMON_ARG_FIELDS, "dryRun", "timeoutMs"]),
  login: new Set([...COMMON_ARG_FIELDS, ...RUNTIME_ARG_FIELDS]),
  "upload-video": new Set([
    ...COMMON_ARG_FIELDS,
    ...EXECUTION_ARG_FIELDS,
    ...RUNTIME_ARG_FIELDS,
    ...UPLOAD_VIDEO_ARG_FIELDS,
  ]),
  "upload-note": new Set([
    ...COMMON_ARG_FIELDS,
    ...EXECUTION_ARG_FIELDS,
    ...RUNTIME_ARG_FIELDS,
    ...UPLOAD_NOTE_ARG_FIELDS,
  ]),
});

function listPlatforms() {
  return Object.entries(PLATFORMS).map(([id, platform]) => ({
    id,
    label: platform.label,
    cli: platform.cli,
    supports: platform.supports,
    extraVideoOptions: platform.extraVideoOptions,
  }));
}

function createStructuredError(code, message, details = {}) {
  const error = new Error(message);
  error.code = code;
  error.details = sanitizeValue(details);
  return error;
}

function sanitizeValue(value) {
  if (Array.isArray(value)) {
    return value.map((item) => sanitizeValue(item));
  }
  if (value && typeof value === "object") {
    const sanitized = {};
    for (const [key, child] of Object.entries(value)) {
      sanitized[key] = SECRET_KEY_PATTERN.test(key) ? "[REDACTED]" : sanitizeValue(child);
    }
    return sanitized;
  }
  if (typeof value === "string") {
    return value.replace(/(password|passwd|token|secret|authorization|cookie)=([^&\s]+)/gi, "$1=[REDACTED]");
  }
  return value;
}

function serializeCommand(command) {
  return command.map((part) => quoteShellArg(String(part))).join(" ");
}

function quoteShellArg(value) {
  if (/^[A-Za-z0-9_./:=,@%+-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, "'\\''")}'`;
}

function getSauBin(args = {}) {
  assertNoSecrets(args);
  const sauBin = args.sauBin || args.sau_bin || process.env.SOCIAL_CONNECTION_SAU_BIN || DEFAULT_SAU_BIN;
  assertNonEmptyString(sauBin, "sauBin");
  return sauBin;
}

function assertNoSecrets(value, path = "") {
  if (!value || typeof value !== "object") {
    return;
  }
  for (const [key, child] of Object.entries(value)) {
    const childPath = path ? `${path}.${key}` : key;
    if (SECRET_KEY_PATTERN.test(key)) {
      throw createStructuredError(
        MCP_ERROR_CODES.invalidParams,
        `Sensitive field is not accepted: ${childPath}`,
        { field: childPath }
      );
    }
    assertNoSecrets(child, childPath);
  }
}

function normalizePlatform(rawPlatform) {
  assertNonEmptyString(rawPlatform, "platform");
  const platform = rawPlatform.toLowerCase();
  if (!PLATFORMS[platform]) {
    throw createStructuredError(
      MCP_ERROR_CODES.unsupportedPlatform,
      `Unsupported platform: ${rawPlatform}`,
      { platform: rawPlatform, supportedPlatforms: Object.keys(PLATFORMS) }
    );
  }
  return platform;
}

function assertOperation(platform, operation) {
  const supportKey = operation === "upload-video" ? "uploadVideo" : operation === "upload-note" ? "uploadNote" : operation;
  if (!PLATFORMS[platform].supports[supportKey]) {
    throw createStructuredError(
      MCP_ERROR_CODES.unsupportedOperation,
      `${PLATFORMS[platform].label} does not support ${operation} through sau CLI`,
      { platform, operation }
    );
  }
}

function assertNonEmptyString(value, field) {
  if (typeof value !== "string" || value.trim() === "") {
    throw createStructuredError(MCP_ERROR_CODES.invalidParams, `Missing or invalid field: ${field}`, { field });
  }
}

function optionalString(value, field) {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  if (typeof value !== "string") {
    throw createStructuredError(MCP_ERROR_CODES.invalidParams, `Invalid field: ${field}`, { field });
  }
  return value;
}

function normalizeTags(rawTags) {
  if (rawTags === undefined || rawTags === null || rawTags === "") {
    return [];
  }
  if (typeof rawTags === "string") {
    return rawTags
      .split(",")
      .map((tag) => tag.trim().replace(/^#/, ""))
      .filter(Boolean);
  }
  if (Array.isArray(rawTags)) {
    return rawTags.map((tag, index) => {
      if (typeof tag !== "string" || tag.trim() === "") {
        throw createStructuredError(MCP_ERROR_CODES.invalidParams, `Invalid tag at index ${index}`, { index });
      }
      return tag.trim().replace(/^#/, "");
    });
  }
  throw createStructuredError(MCP_ERROR_CODES.invalidParams, "tags must be a string or array of strings", {
    field: "tags",
  });
}

function normalizeImages(rawImages) {
  if (!Array.isArray(rawImages) || rawImages.length === 0) {
    throw createStructuredError(MCP_ERROR_CODES.invalidParams, "images must be a non-empty string array", {
      field: "images",
    });
  }
  return rawImages.map((image, index) => {
    assertNonEmptyString(image, `images[${index}]`);
    return image;
  });
}

function appendRuntimeFlags(argv, args, platform) {
  if (!PLATFORMS[platform].supports.headless) {
    rejectUnsupportedArgs(args, platform, ["headless", "headed", "debug"], "runtime flags");
    return;
  }
  if (args.headed === true) {
    argv.push("--headed");
  } else if (args.headless === true) {
    argv.push("--headless");
  }
  if (args.debug === true) {
    argv.push("--debug");
  }
}

function appendTags(argv, rawTags, platform) {
  const tags = normalizeTags(rawTags);
  const maxTags = PLATFORMS[platform].supports.maxTags;
  if (maxTags && tags.length > maxTags) {
    throw createStructuredError(
      MCP_ERROR_CODES.invalidParams,
      `${PLATFORMS[platform].label} supports at most ${maxTags} tags`,
      { platform, maxTags, provided: tags.length }
    );
  }
  if (tags.length > 0) {
    argv.push("--tags", tags.join(","));
  }
  return tags;
}

function appendOptionalStringArg(argv, flag, value, field) {
  const normalized = optionalString(value, field);
  if (normalized !== undefined) {
    argv.push(flag, normalized);
  }
}

function buildCommand(args = {}) {
  assertNoSecrets(args);
  const operation = args.operation || args.action;
  assertNonEmptyString(operation, "operation");
  if (!["check", "login", "upload-video", "upload-note"].includes(operation)) {
    throw createStructuredError(MCP_ERROR_CODES.unsupportedOperation, `Unsupported operation: ${operation}`, {
      operation,
    });
  }
  assertKnownArgs(args, operation);
  const platform = normalizePlatform(args.platform);
  assertOperation(platform, operation);
  const sauBin = getSauBin(args);
  const argv = [sauBin, PLATFORMS[platform].cli, operation];
  appendCommonOperationArgs(argv, platform, operation, args);
  return makePlan(argv, {
    platform,
    operation,
    dryRun: true,
    willExecute: false,
    safety: "plan-only",
  });
}

function assertKnownArgs(args, operation) {
  const allowedFields = ALLOWED_FIELDS_BY_OPERATION[operation];
  for (const field of Object.keys(args)) {
    if (!allowedFields.has(field)) {
      throw createStructuredError(
        MCP_ERROR_CODES.invalidParams,
        `Unsupported argument for ${operation}: ${field}`,
        { operation, field, allowedFields: Array.from(allowedFields).sort() }
      );
    }
  }
}

function appendCommonOperationArgs(argv, platform, operation, args) {
  assertNonEmptyString(args.account, "account");
  argv.push("--account", args.account);

  if (operation === "check") {
    return;
  }

  if (operation === "login") {
    rejectUnsupportedArgs(args, platform, ["dryRun", "confirm"], "login");
    appendRuntimeFlags(argv, args, platform);
    return;
  }

  if (operation === "upload-video") {
    validateUploadVideoArgs(platform, args);
    assertNonEmptyString(args.file, "file");
    assertNonEmptyString(args.title, "title");
    argv.push("--file", args.file, "--title", args.title);
    if (platform === "bilibili") {
      assertNonEmptyString(args.desc, "desc");
      argv.push("--desc", args.desc);
      if (!Number.isInteger(args.tid) && typeof args.tid !== "string") {
        throw createStructuredError(MCP_ERROR_CODES.invalidParams, "Bilibili upload_video requires tid", {
          field: "tid",
        });
      }
      argv.push("--tid", String(args.tid));
    } else {
      appendOptionalStringArg(argv, "--desc", args.desc, "desc");
    }
    appendTags(argv, args.tags, platform);
    appendUploadVideoOptions(argv, platform, args);
    appendRuntimeFlags(argv, args, platform);
    return;
  }

  if (operation === "upload-note") {
    const images = validateUploadNoteArgs(platform, args);
    assertNonEmptyString(args.title, "title");
    argv.push("--images", ...images, "--title", args.title);
    appendOptionalStringArg(argv, "--note", args.note, "note");
    appendTags(argv, args.tags, platform);
    appendOptionalStringArg(argv, "--schedule", args.schedule, "schedule");
    if (platform === "douyin") {
      appendOptionalStringArg(argv, "--bgm", args.bgm, "bgm");
    }
    appendRuntimeFlags(argv, args, platform);
  }
}

function rejectUnsupportedArgs(args, platform, fields, context) {
  for (const field of fields) {
    if (args[field] !== undefined && args[field] !== null && args[field] !== false && args[field] !== "") {
      throw createStructuredError(
        MCP_ERROR_CODES.unsupportedOperation,
        `${PLATFORMS[platform].label} does not support ${field} for ${context}`,
        { platform, field, context }
      );
    }
  }
}

function validateUploadVideoArgs(platform, args) {
  if (args.schedule && !PLATFORMS[platform].supports.schedule) {
    throw createStructuredError(
      MCP_ERROR_CODES.unsupportedOperation,
      `${PLATFORMS[platform].label} does not support schedule for upload-video`,
      { platform, field: "schedule", operation: "upload-video" }
    );
  }
  if (!["douyin", "tencent"].includes(platform)) {
    rejectUnsupportedArgs(args, platform, ["thumbnailLandscape", "thumbnailPortrait"], "upload-video");
  }
  if (!["douyin", "kuaishou", "xiaohongshu", "tencent", "youtube"].includes(platform)) {
    rejectUnsupportedArgs(args, platform, ["thumbnail"], "upload-video");
  }
  if (platform !== "douyin") {
    rejectUnsupportedArgs(args, platform, ["productLink", "productTitle"], "upload-video");
  }
  if (platform !== "bilibili") {
    rejectUnsupportedArgs(args, platform, ["tid"], "upload-video");
  }
  if (platform !== "tencent") {
    rejectUnsupportedArgs(args, platform, ["shortTitle", "category", "draft"], "upload-video");
  }
  if (platform !== "youtube") {
    rejectUnsupportedArgs(args, platform, ["playlist", "visibility"], "upload-video");
  } else if (
    args.visibility !== undefined &&
    args.visibility !== null &&
    !["public", "unlisted", "private"].includes(args.visibility)
  ) {
    throw createStructuredError(MCP_ERROR_CODES.invalidParams, "Invalid YouTube visibility", {
      platform,
      field: "visibility",
      allowed: ["public", "unlisted", "private"],
    });
  }
}

function validateUploadNoteArgs(platform, args) {
  rejectUnsupportedArgs(args, platform, ["file", "desc", "thumbnail", "thumbnailLandscape", "thumbnailPortrait"], "upload-note");
  if (platform !== "douyin") {
    rejectUnsupportedArgs(args, platform, ["bgm"], "upload-note");
  }
  const images = normalizeImages(args.images);
  const maxNoteImages = PLATFORMS[platform].supports.maxNoteImages;
  if (maxNoteImages && images.length > maxNoteImages) {
    throw createStructuredError(
      MCP_ERROR_CODES.invalidParams,
      `${PLATFORMS[platform].label} supports at most ${maxNoteImages} note images`,
      { platform, field: "images", maxImages: maxNoteImages, provided: images.length }
    );
  }
  if (PLATFORMS[platform].supports.disallowGifNoteImages) {
    const gifImage = images.find((image) => path.extname(image).toLowerCase() === ".gif");
    if (gifImage) {
      throw createStructuredError(
        MCP_ERROR_CODES.invalidParams,
        `${PLATFORMS[platform].label} upload-note does not support GIF images`,
        { platform, field: "images", image: gifImage }
      );
    }
  }
  return images;
}

function appendUploadVideoOptions(argv, platform, args) {
  appendOptionalStringArg(argv, "--schedule", args.schedule, "schedule");
  if (["douyin", "kuaishou", "xiaohongshu", "tencent", "youtube"].includes(platform)) {
    appendOptionalStringArg(argv, "--thumbnail", args.thumbnail, "thumbnail");
  }
  if (["douyin", "tencent"].includes(platform)) {
    appendOptionalStringArg(argv, "--thumbnail-landscape", args.thumbnailLandscape, "thumbnailLandscape");
    appendOptionalStringArg(argv, "--thumbnail-portrait", args.thumbnailPortrait, "thumbnailPortrait");
  }
  if (platform === "douyin") {
    appendOptionalStringArg(argv, "--product-link", args.productLink, "productLink");
    appendOptionalStringArg(argv, "--product-title", args.productTitle, "productTitle");
  }
  if (platform === "tencent") {
    appendOptionalStringArg(argv, "--short-title", args.shortTitle, "shortTitle");
    appendOptionalStringArg(argv, "--category", args.category, "category");
    if (args.draft === true) {
      argv.push("--draft");
    }
  }
  if (platform === "youtube") {
    appendOptionalStringArg(argv, "--playlist", args.playlist, "playlist");
    appendOptionalStringArg(argv, "--visibility", args.visibility, "visibility");
  }
}

function makePlan(argv, meta) {
  return {
    ...meta,
    command: argv,
    shellCommand: serializeCommand(argv),
  };
}

async function loginPrepare(args = {}) {
  const plan = buildCommand({ ...args, operation: "login" });
  return {
    ...plan,
    dryRun: true,
    willExecute: false,
    guidance: loginGuidance(plan.platform, args.account),
  };
}

function loginGuidance(platform, account) {
  if (platform === "bilibili") {
    return [
      "Bilibili login requires an interactive local terminal.",
      `Run the command yourself and scan qrcode.png if the terminal QR code is incomplete.`,
      `Account file will be managed by sau for account "${account}".`,
    ];
  }
  return [
    "Run this command in a local terminal where the browser or QR-code flow can be completed.",
    "If sau generates a QR-code image, show/open that local image for the user to scan.",
    `Account file will be managed by sau for account "${account}".`,
  ];
}

async function checkAccount(args = {}, runner = runCommand) {
  const plan = buildCommand({ ...args, operation: "check" });
  const dryRun = args.dryRun === true;
  if (dryRun) {
    return { ...plan, dryRun: true, willExecute: false };
  }
  const result = await executeRunner(plan, args, runner);
  return {
    ...plan,
    dryRun: false,
    willExecute: true,
    exitCode: result.exitCode,
    ok: result.exitCode === 0,
    stdout: sanitizeValue(result.stdout),
    stderr: sanitizeValue(result.stderr),
  };
}

async function uploadVideo(args = {}, runner = runCommand) {
  return uploadWithGate({ ...args, operation: "upload-video" }, runner);
}

async function uploadNote(args = {}, runner = runCommand) {
  return uploadWithGate({ ...args, operation: "upload-note" }, runner);
}

async function uploadWithGate(args, runner) {
  const plan = buildCommand(args);
  const dryRun = args.dryRun !== false;
  const confirm = args.confirm === true;
  if (dryRun || !confirm) {
    if (!dryRun) {
      throw createStructuredError(
        MCP_ERROR_CODES.publishConfirmationRequired,
        "confirm=true is required when dryRun=false",
        { dryRun, confirm, operation: args.operation, platform: plan.platform }
      );
    }
    return {
      ...plan,
      dryRun,
      confirm,
      willExecute: false,
      safety: "dry-run default: upload command was not executed",
    };
  }
  const result = await executeRunner(plan, args, runner);
  if (result.exitCode !== 0) {
    throw createStructuredError(MCP_ERROR_CODES.commandFailed, "sau upload command failed", {
      exitCode: result.exitCode,
      stdout: result.stdout,
      stderr: result.stderr,
    });
  }
  return {
    ...plan,
    dryRun: false,
    confirm: true,
    willExecute: true,
    exitCode: result.exitCode,
    ok: true,
    stdout: sanitizeValue(result.stdout),
    stderr: sanitizeValue(result.stderr),
  };
}

async function executeRunner(plan, args, runner) {
  try {
    return await runner(plan.command, { timeoutMs: args.timeoutMs });
  } catch (error) {
    throw createStructuredError(MCP_ERROR_CODES.commandFailed, "sau command failed to start or complete", {
      platform: plan.platform,
      operation: plan.operation,
      message: error && error.message ? error.message : String(error),
      code: error && error.code,
    });
  }
}

function runCommand(command, options = {}) {
  return new Promise((resolve, reject) => {
    const [bin, ...args] = command;
    const child = spawn(bin, args, {
      stdio: ["ignore", "pipe", "pipe"],
      env: process.env,
    });
    let stdout = "";
    let stderr = "";
    let finished = false;
    const timeoutMs = Number.isInteger(options.timeoutMs) ? options.timeoutMs : 15 * 60 * 1000;
    const timer = setTimeout(() => {
      if (!finished) {
        child.kill("SIGTERM");
      }
    }, timeoutMs);

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (exitCode, signal) => {
      finished = true;
      clearTimeout(timer);
      resolve({
        exitCode: exitCode === null ? 1 : exitCode,
        signal,
        stdout,
        stderr,
      });
    });
  });
}

module.exports = {
  MCP_ERROR_CODES,
  PLATFORMS,
  buildCommand,
  checkAccount,
  createStructuredError,
  listPlatforms,
  loginPrepare,
  runCommand,
  sanitizeValue,
  serializeCommand,
  uploadNote,
  uploadVideo,
};
