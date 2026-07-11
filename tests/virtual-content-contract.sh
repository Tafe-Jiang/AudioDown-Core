#!/bin/sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
fixture_root="$root_dir/test-fixtures/repositories/virtual"

command -v node >/dev/null 2>&1 || {
  printf '%s\n' "VIRTUAL_CONTENT_CONTRACT: Node.js is required" >&2
  exit 1
}

node --input-type=module - "$root_dir" "$fixture_root" <<'NODE'
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { pathToFileURL } from "node:url";

const [rootDir, fixtureRoot] = process.argv.slice(2);
const sdkPath = path.join(rootDir, "plugin-sdk/node/src/index.js");
const fixtures = [
  {
    directory: "virtual-content",
    id: "com.audiodown.virtual.content",
    platformId: "virtual",
  },
  {
    directory: "virtual-content-backup",
    id: "com.audiodown.virtual.content-backup",
    platformId: "virtual",
  },
  {
    directory: "virtual-catalog",
    id: "com.audiodown.catalog.content",
    platformId: "catalog",
  },
];
const contentCapabilities = [
  "content.search",
  "content.discover",
  "content.categories",
  "content.album.get",
  "content.tracks.list",
];

for (const fixture of fixtures) {
  const pluginRoot = path.join(fixtureRoot, "plugins", fixture.directory);
  const manifestPath = path.join(pluginRoot, "audiodown-plugin.json");
  assert.equal(fs.existsSync(manifestPath), true, `${fixture.directory} manifest`);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.equal(manifest.id, fixture.id);
  assert.equal(manifest.platform.id, fixture.platformId);
  assert.deepEqual(manifest.network.allowedHosts, []);
  for (const capability of contentCapabilities) {
    assert.equal(manifest.capabilities.includes(capability), true);
  }

  const serializedFixture = [
    fs.readFileSync(manifestPath, "utf8"),
    fs.readFileSync(path.join(pluginRoot, "src/index.js"), "utf8"),
  ].join("\n");
  assert.equal(serializedFixture.includes("://"), false);

  const build = spawnSync("npm", ["run", "build"], {
    cwd: pluginRoot,
    encoding: "utf8",
  });
  assert.equal(build.status, 0, build.stderr || build.stdout);
}

const sessions = new Map();
for (const fixture of fixtures) {
  sessions.set(fixture.id, await startFixture(fixture));
}

try {
  const primary = sessions.get("com.audiodown.virtual.content");
  const backup = sessions.get("com.audiodown.virtual.content-backup");
  const catalog = sessions.get("com.audiodown.catalog.content");

  for (const [pluginId, session] of sessions) {
    const hello = await session.call("system.hello", {});
    assert.equal(hello.pluginId, pluginId);
    const search = await session.call("content.search", {
      query: "fixture",
      limit: 20,
    });
    assert.equal(search.items.length > 0, true);
    assert.equal(typeof search.items[0].resourceId, "string");

    const categories = await session.call("content.categories", {});
    assert.equal(categories.items.length > 0, true);

    const album = await session.call("content.album.get", {
      resourceId: search.items[0].resourceId,
    });
    assert.equal(album.album.resourceId, search.items[0].resourceId);

    const tracksPageOne = await session.call("content.tracks.list", {
      albumResourceId: search.items[0].resourceId,
      limit: 1,
    });
    assert.equal(tracksPageOne.items.length, 1);
    assert.equal(typeof tracksPageOne.nextCursor, "string");
    const tracksPageTwo = await session.call("content.tracks.list", {
      albumResourceId: search.items[0].resourceId,
      cursor: tracksPageOne.nextCursor,
      limit: 1,
    });
    assert.equal(tracksPageTwo.items.length, 1);
    assert.equal(tracksPageTwo.nextCursor, undefined);
  }

  const discover = await primary.call("content.discover", { limit: 20 });
  assert.deepEqual(
    discover.sections.map((section) => section.layout),
    [
      "hero-carousel",
      "album-grid",
      "horizontal-list",
      "ranked-list",
      "category-grid",
    ],
  );

  const primarySearch = await primary.call("content.search", {
    query: "fixture",
    limit: 20,
  });
  const catalogSearch = await catalog.call("content.search", {
    query: "fixture",
    limit: 20,
  });
  assert.equal(
    primarySearch.items[0].canonicalId,
    catalogSearch.items[0].canonicalId,
  );

  const retryable = await primary.callError("content.search", {
    query: "__retryable__",
    limit: 20,
  });
  assert.equal(retryable.data.code, "RATE_LIMITED");
  assert.equal(retryable.data.retryAfterSeconds, 1);

  const hardFailure = await primary.callError("content.search", {
    query: "__hard_failure__",
    limit: 20,
  });
  assert.equal(hardFailure.data.code, "RESOURCE_ACCESS_DENIED");
  assert.equal(hardFailure.data.summary, "Virtual resource access was denied");
  assert.equal(JSON.stringify(hardFailure).includes("stack"), false);

  const delayedAt = Date.now();
  const delayed = await primary.call("content.search", {
    query: "__delay__",
    limit: 20,
  });
  assert.equal(Date.now() - delayedAt >= 100, true);
  assert.equal(delayed.items.length, 1);

  const backupSearch = await backup.call("content.search", {
    query: "__retryable__",
    limit: 20,
  });
  assert.equal(backupSearch.items.length, 1);
} finally {
  await Promise.all([...sessions.values()].map((session) => session.close()));
}

async function startFixture(fixture) {
  const pluginRoot = path.join(fixtureRoot, "plugins", fixture.directory);
  const child = spawn(process.execPath, [path.join(pluginRoot, "src/index.js")], {
    cwd: pluginRoot,
    env: {
      ...process.env,
      AUDIODOWN_NODE_SDK_PATH: pathToFileURL(sdkPath).href,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const lines = readline.createInterface({ input: child.stdout });
  const pending = new Map();
  let nextId = 1;
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr += chunk.toString("utf8");
  });
  lines.on("line", (line) => {
    const response = JSON.parse(line);
    const waiter = pending.get(response.id);
    if (waiter) {
      pending.delete(response.id);
      waiter.resolve(response);
    }
  });
  child.once("exit", (code) => {
    for (const waiter of pending.values()) {
      waiter.reject(new Error(`fixture exited ${code}: ${stderr}`));
    }
    pending.clear();
  });

  async function exchange(method, params) {
    const id = `contract-${nextId++}`;
    const response = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`fixture RPC timed out: ${fixture.id} ${method}`));
      }, 3000);
      pending.set(id, {
        resolve(value) {
          clearTimeout(timeout);
          resolve(value);
        },
        reject(error) {
          clearTimeout(timeout);
          reject(error);
        },
      });
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return response;
  }

  return {
    async call(method, params) {
      const response = await exchange(method, params);
      assert.equal(response.error, undefined, JSON.stringify(response.error));
      return response.result;
    },
    async callError(method, params) {
      const response = await exchange(method, params);
      assert.equal(response.result, undefined);
      assert.equal(response.error.code, -32000);
      return response.error;
    },
    async close() {
      if (child.exitCode !== null) {
        return;
      }
      await exchange("system.shutdown", {});
      await new Promise((resolve) => child.once("exit", resolve));
    },
  };
}
NODE

printf '%s\n' "Virtual content SDK contract passed"
