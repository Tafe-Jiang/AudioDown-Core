import assert from "node:assert/strict";
import test from "node:test";

import {
  CONTENT_METHODS,
  ContentContractError,
  PluginContentError,
  createContentHandlers,
} from "../src/index.js";

test("exports only the five phase-three content methods", () => {
  assert.deepEqual(Object.values(CONTENT_METHODS), [
    "content.search",
    "content.discover",
    "content.categories",
    "content.album.get",
    "content.tracks.list",
  ]);
  assert.equal(Object.isFrozen(CONTENT_METHODS), true);
});

test("rejects handler names outside the content allowlist", () => {
  assert.throws(
    () => createContentHandlers({ echo: async () => ({}) }),
    (error) =>
      error instanceof ContentContractError &&
      error.code === "INVALID_HANDLER",
  );
  assert.throws(
    () =>
      createContentHandlers({
        [CONTENT_METHODS.SEARCH]: "not a function",
      }),
    (error) =>
      error instanceof ContentContractError &&
      error.code === "INVALID_HANDLER",
  );
});

test("validates search input and normalizes a valid result", async () => {
  const handlers = createContentHandlers({
    [CONTENT_METHODS.SEARCH]: async ({ query, cursor, limit }) => ({
      items: [
        {
          resourceType: "album",
          resourceId: `${query}-${limit}`,
          canonicalId: "virtual:album:1",
          title: "Virtual Album",
          subtitle: cursor,
        },
      ],
      nextCursor: "opaque-next",
    }),
  });

  const result = await handlers[CONTENT_METHODS.SEARCH]({
    query: "virtual",
    cursor: "opaque-current",
    limit: 20,
  });
  assert.equal(result.items[0].resourceId, "virtual-20");
  assert.equal(result.nextCursor, "opaque-next");

  await assert.rejects(
    handlers[CONTENT_METHODS.SEARCH]({
      query: "x".repeat(513),
      limit: 20,
    }),
    (error) =>
      error instanceof ContentContractError &&
      error.code === "INVALID_REQUEST",
  );
});

test("validates opaque resources, cursors, and response shapes", async () => {
  const handlers = createContentHandlers({
    [CONTENT_METHODS.ALBUM_GET]: async () => ({
      album: {
        resourceId: "x".repeat(1025),
        title: "Oversized album",
      },
    }),
    [CONTENT_METHODS.TRACKS_LIST]: async () => ({
      items: [],
      nextCursor: "x".repeat(4097),
    }),
  });

  await assert.rejects(
    handlers[CONTENT_METHODS.ALBUM_GET]({ resourceId: "album-1" }),
    (error) =>
      error instanceof ContentContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );
  await assert.rejects(
    handlers[CONTENT_METHODS.TRACKS_LIST]({
      albumResourceId: "album-1",
      limit: 20,
    }),
    (error) =>
      error instanceof ContentContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );
});

test("creates bounded standard plugin errors", () => {
  const error = new PluginContentError(
    "RATE_LIMITED",
    "The virtual source asked Core to retry later",
    30,
  );
  assert.equal(error.code, "RATE_LIMITED");
  assert.equal(error.retryAfterSeconds, 30);

  assert.throws(
    () => new PluginContentError("NOT_A_CODE", "unsafe"),
    (candidate) =>
      candidate instanceof ContentContractError &&
      candidate.code === "INVALID_ERROR",
  );
  assert.throws(
    () => new PluginContentError("PLUGIN_INTERNAL_ERROR", "x".repeat(513)),
    (candidate) =>
      candidate instanceof ContentContractError &&
      candidate.code === "INVALID_ERROR",
  );
});
