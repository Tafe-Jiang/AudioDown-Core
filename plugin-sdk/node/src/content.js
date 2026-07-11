const MAX_QUERY_BYTES = 512;
const MAX_CURSOR_BYTES = 4 * 1024;
const MAX_OPAQUE_ID_BYTES = 1024;
const MAX_ITEMS = 200;
const MAX_SECTIONS = 32;
const MAX_TITLE_BYTES = 512;
const MAX_SHORT_TEXT_BYTES = 1024;
const MAX_DESCRIPTION_BYTES = 4 * 1024;
const MAX_ERROR_SUMMARY_BYTES = 512;
const MAX_RETRY_AFTER_SECONDS = 24 * 60 * 60;

export const CONTENT_METHODS = Object.freeze({
  SEARCH: "content.search",
  DISCOVER: "content.discover",
  CATEGORIES: "content.categories",
  ALBUM_GET: "content.album.get",
  TRACKS_LIST: "content.tracks.list",
});

const CONTENT_METHOD_SET = new Set(Object.values(CONTENT_METHODS));
const RESOURCE_TYPES = new Set(["album", "track", "category"]);
const DISCOVER_LAYOUTS = new Set([
  "hero-carousel",
  "album-grid",
  "horizontal-list",
  "ranked-list",
  "category-grid",
]);
const STANDARD_ERROR_CODES = new Set([
  "INVALID_REQUEST",
  "PLUGIN_NOT_FOUND",
  "PLUGIN_DISABLED",
  "PLUGIN_CAPABILITY_MISSING",
  "PLUGIN_UNAVAILABLE",
  "PLUGIN_TIMEOUT",
  "PLUGIN_RESPONSE_INVALID",
  "RESOURCE_NOT_FOUND",
  "RESOURCE_ACCESS_DENIED",
  "RESOURCE_TEMPORARILY_UNAVAILABLE",
  "RATE_LIMITED",
  "PLATFORM_RESPONSE_CHANGED",
  "PLUGIN_INTERNAL_ERROR",
]);

export class ContentContractError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "ContentContractError";
    this.code = code;
  }
}

export class PluginContentError extends Error {
  constructor(code, summary, retryAfterSeconds = undefined) {
    validatePluginError(code, summary, retryAfterSeconds);
    super(summary);
    this.name = "PluginContentError";
    this.code = code;
    this.summary = summary;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

export function createContentHandlers(handlers) {
  assertPlainObject(handlers, "handlers", "INVALID_HANDLER");
  const wrapped = {};

  for (const [method, handler] of Object.entries(handlers)) {
    if (!CONTENT_METHOD_SET.has(method) || typeof handler !== "function") {
      throw new ContentContractError(
        "INVALID_HANDLER",
        "Content handler method is not allowed",
      );
    }
    wrapped[method] = async (params) => {
      validateRequest(method, params);
      const result = await handler(params);
      validateResult(method, result);
      return result;
    };
  }

  return Object.freeze(wrapped);
}

export function isContentMethod(method) {
  return CONTENT_METHOD_SET.has(method);
}

function validateRequest(method, params) {
  assertPlainObject(params, "params", "INVALID_REQUEST");
  switch (method) {
    case CONTENT_METHODS.SEARCH:
      assertKeys(params, ["query", "cursor", "limit"], "INVALID_REQUEST");
      assertText(params.query, MAX_QUERY_BYTES, "query", "INVALID_REQUEST");
      assertOptionalOpaque(
        params.cursor,
        MAX_CURSOR_BYTES,
        "cursor",
        "INVALID_REQUEST",
      );
      assertLimit(params.limit, "INVALID_REQUEST");
      break;
    case CONTENT_METHODS.DISCOVER:
      assertKeys(params, ["cursor", "limit"], "INVALID_REQUEST");
      assertOptionalOpaque(
        params.cursor,
        MAX_CURSOR_BYTES,
        "cursor",
        "INVALID_REQUEST",
      );
      assertLimit(params.limit, "INVALID_REQUEST");
      break;
    case CONTENT_METHODS.CATEGORIES:
      assertKeys(params, [], "INVALID_REQUEST");
      break;
    case CONTENT_METHODS.ALBUM_GET:
      assertKeys(params, ["resourceId"], "INVALID_REQUEST");
      assertOpaque(
        params.resourceId,
        MAX_OPAQUE_ID_BYTES,
        "resourceId",
        "INVALID_REQUEST",
      );
      break;
    case CONTENT_METHODS.TRACKS_LIST:
      assertKeys(
        params,
        ["albumResourceId", "cursor", "limit"],
        "INVALID_REQUEST",
      );
      assertOpaque(
        params.albumResourceId,
        MAX_OPAQUE_ID_BYTES,
        "albumResourceId",
        "INVALID_REQUEST",
      );
      assertOptionalOpaque(
        params.cursor,
        MAX_CURSOR_BYTES,
        "cursor",
        "INVALID_REQUEST",
      );
      assertLimit(params.limit, "INVALID_REQUEST");
      break;
    default:
      throw new ContentContractError(
        "INVALID_HANDLER",
        "Content handler method is not allowed",
      );
  }
}

function validateResult(method, result) {
  assertPlainObject(result, "result", "PLUGIN_RESPONSE_INVALID");
  switch (method) {
    case CONTENT_METHODS.SEARCH:
      assertKeys(result, ["items", "nextCursor"], "PLUGIN_RESPONSE_INVALID");
      assertContentItems(result.items);
      assertOptionalOpaque(
        result.nextCursor,
        MAX_CURSOR_BYTES,
        "nextCursor",
        "PLUGIN_RESPONSE_INVALID",
      );
      break;
    case CONTENT_METHODS.DISCOVER:
      assertKeys(result, ["sections", "nextCursor"], "PLUGIN_RESPONSE_INVALID");
      assertArray(result.sections, MAX_SECTIONS, "sections");
      for (const section of result.sections) {
        assertDiscoverSection(section);
      }
      assertOptionalOpaque(
        result.nextCursor,
        MAX_CURSOR_BYTES,
        "nextCursor",
        "PLUGIN_RESPONSE_INVALID",
      );
      break;
    case CONTENT_METHODS.CATEGORIES:
      assertKeys(result, ["items"], "PLUGIN_RESPONSE_INVALID");
      assertArray(result.items, MAX_ITEMS, "items");
      for (const item of result.items) {
        assertCategory(item);
      }
      break;
    case CONTENT_METHODS.ALBUM_GET:
      assertKeys(result, ["album"], "PLUGIN_RESPONSE_INVALID");
      assertAlbum(result.album);
      break;
    case CONTENT_METHODS.TRACKS_LIST:
      assertKeys(result, ["items", "nextCursor"], "PLUGIN_RESPONSE_INVALID");
      assertArray(result.items, MAX_ITEMS, "items");
      for (const item of result.items) {
        assertTrack(item);
      }
      assertOptionalOpaque(
        result.nextCursor,
        MAX_CURSOR_BYTES,
        "nextCursor",
        "PLUGIN_RESPONSE_INVALID",
      );
      break;
    default:
      throw new ContentContractError(
        "INVALID_HANDLER",
        "Content handler method is not allowed",
      );
  }
}

function assertContentItems(items) {
  assertArray(items, MAX_ITEMS, "items");
  for (const item of items) {
    assertPlainObject(item, "item", "PLUGIN_RESPONSE_INVALID");
    assertKeys(
      item,
      [
        "resourceType",
        "resourceId",
        "canonicalId",
        "title",
        "subtitle",
        "description",
      ],
      "PLUGIN_RESPONSE_INVALID",
    );
    if (!RESOURCE_TYPES.has(item.resourceType)) {
      invalid("resourceType", "PLUGIN_RESPONSE_INVALID");
    }
    assertOpaque(
      item.resourceId,
      MAX_OPAQUE_ID_BYTES,
      "resourceId",
      "PLUGIN_RESPONSE_INVALID",
    );
    assertOptionalOpaque(
      item.canonicalId,
      MAX_OPAQUE_ID_BYTES,
      "canonicalId",
      "PLUGIN_RESPONSE_INVALID",
    );
    assertText(
      item.title,
      MAX_TITLE_BYTES,
      "title",
      "PLUGIN_RESPONSE_INVALID",
    );
    assertOptionalText(
      item.subtitle,
      MAX_SHORT_TEXT_BYTES,
      "subtitle",
      "PLUGIN_RESPONSE_INVALID",
    );
    assertOptionalText(
      item.description,
      MAX_DESCRIPTION_BYTES,
      "description",
      "PLUGIN_RESPONSE_INVALID",
    );
  }
}

function assertDiscoverSection(section) {
  assertPlainObject(section, "section", "PLUGIN_RESPONSE_INVALID");
  assertKeys(
    section,
    ["id", "title", "layout", "items"],
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOpaque(
    section.id,
    MAX_OPAQUE_ID_BYTES,
    "section.id",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertText(
    section.title,
    MAX_TITLE_BYTES,
    "section.title",
    "PLUGIN_RESPONSE_INVALID",
  );
  if (!DISCOVER_LAYOUTS.has(section.layout)) {
    invalid("section.layout", "PLUGIN_RESPONSE_INVALID");
  }
  assertContentItems(section.items);
}

function assertCategory(item) {
  assertPlainObject(item, "category", "PLUGIN_RESPONSE_INVALID");
  assertKeys(
    item,
    ["resourceId", "canonicalId", "title", "description"],
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOpaque(
    item.resourceId,
    MAX_OPAQUE_ID_BYTES,
    "resourceId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalOpaque(
    item.canonicalId,
    MAX_OPAQUE_ID_BYTES,
    "canonicalId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertText(
    item.title,
    MAX_TITLE_BYTES,
    "title",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalText(
    item.description,
    MAX_DESCRIPTION_BYTES,
    "description",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function assertAlbum(album) {
  assertPlainObject(album, "album", "PLUGIN_RESPONSE_INVALID");
  assertKeys(
    album,
    [
      "resourceId",
      "canonicalId",
      "title",
      "creator",
      "description",
      "trackCount",
    ],
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOpaque(
    album.resourceId,
    MAX_OPAQUE_ID_BYTES,
    "resourceId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalOpaque(
    album.canonicalId,
    MAX_OPAQUE_ID_BYTES,
    "canonicalId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertText(
    album.title,
    MAX_TITLE_BYTES,
    "title",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalText(
    album.creator,
    MAX_SHORT_TEXT_BYTES,
    "creator",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalText(
    album.description,
    MAX_DESCRIPTION_BYTES,
    "description",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalNonnegativeInteger(
    album.trackCount,
    "trackCount",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function assertTrack(track) {
  assertPlainObject(track, "track", "PLUGIN_RESPONSE_INVALID");
  assertKeys(
    track,
    [
      "resourceId",
      "canonicalId",
      "title",
      "subtitle",
      "sequence",
      "durationSeconds",
    ],
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOpaque(
    track.resourceId,
    MAX_OPAQUE_ID_BYTES,
    "resourceId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalOpaque(
    track.canonicalId,
    MAX_OPAQUE_ID_BYTES,
    "canonicalId",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertText(
    track.title,
    MAX_TITLE_BYTES,
    "title",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalText(
    track.subtitle,
    MAX_SHORT_TEXT_BYTES,
    "subtitle",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalNonnegativeInteger(
    track.sequence,
    "sequence",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalNonnegativeInteger(
    track.durationSeconds,
    "durationSeconds",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function validatePluginError(code, summary, retryAfterSeconds) {
  if (!STANDARD_ERROR_CODES.has(code)) {
    invalid("error code", "INVALID_ERROR");
  }
  assertText(
    summary,
    MAX_ERROR_SUMMARY_BYTES,
    "error summary",
    "INVALID_ERROR",
  );
  if (
    retryAfterSeconds !== undefined &&
    (!Number.isInteger(retryAfterSeconds) ||
      retryAfterSeconds < 0 ||
      retryAfterSeconds > MAX_RETRY_AFTER_SECONDS)
  ) {
    invalid("retryAfterSeconds", "INVALID_ERROR");
  }
}

function assertPlainObject(value, field, code) {
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.getPrototypeOf(value) !== Object.prototype
  ) {
    invalid(field, code);
  }
}

function assertKeys(value, allowed, code) {
  const allowedSet = new Set(allowed);
  if (Object.keys(value).some((key) => !allowedSet.has(key))) {
    invalid("unknown field", code);
  }
}

function assertArray(value, maximum, field) {
  if (!Array.isArray(value) || value.length > maximum) {
    invalid(field, "PLUGIN_RESPONSE_INVALID");
  }
}

function assertLimit(value, code) {
  if (!Number.isInteger(value) || value < 1 || value > MAX_ITEMS) {
    invalid("limit", code);
  }
}

function assertText(value, maximum, field, code) {
  if (
    typeof value !== "string" ||
    value.trim().length === 0 ||
    Buffer.byteLength(value, "utf8") > maximum
  ) {
    invalid(field, code);
  }
}

function assertOptionalText(value, maximum, field, code) {
  if (value !== undefined) {
    assertText(value, maximum, field, code);
  }
}

function assertOpaque(value, maximum, field, code) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.includes("\0") ||
    Buffer.byteLength(value, "utf8") > maximum
  ) {
    invalid(field, code);
  }
}

function assertOptionalOpaque(value, maximum, field, code) {
  if (value !== undefined) {
    assertOpaque(value, maximum, field, code);
  }
}

function assertOptionalNonnegativeInteger(value, field, code) {
  if (
    value !== undefined &&
    (!Number.isInteger(value) || value < 0 || value > 0xffffffff)
  ) {
    invalid(field, code);
  }
}

function invalid(field, code) {
  throw new ContentContractError(code, `${field} is invalid`);
}
