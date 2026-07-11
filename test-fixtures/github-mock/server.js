"use strict";

const { createReadStream, statSync } = require("node:fs");
const { createServer } = require("node:http");

const commitSha = "0123456789abcdef0123456789abcdef01234567";
const archivePath = process.env.AUDIODOWN_FIXTURE_ARCHIVE;
const port = Number.parseInt(process.env.AUDIODOWN_GITHUB_MOCK_PORT ?? "18082", 10);

if (!archivePath) {
  throw new Error("AUDIODOWN_FIXTURE_ARCHIVE is required");
}

const archiveSize = statSync(archivePath).size;
const routes = new Map([
  [
    "/repos/example-owner/example-repository",
    { type: "json", body: { default_branch: "main" } },
  ],
  [
    "/repos/example-owner/example-repository/commits/main",
    { type: "json", body: { sha: commitSha } },
  ],
  [
    `/example-owner/example-repository/tar.gz/${commitSha}`,
    { type: "archive" },
  ],
]);

const server = createServer((request, response) => {
  const url = new URL(request.url, "http://github-mock.invalid");
  const route = request.method === "GET" ? routes.get(url.pathname) : undefined;
  process.stdout.write(`${request.method} ${url.pathname}\n`);

  if (!route || url.search) {
    response.writeHead(404, { "content-type": "application/json" });
    response.end('{"error":"not found"}\n');
    return;
  }

  if (route.type === "json") {
    const body = `${JSON.stringify(route.body)}\n`;
    response.writeHead(200, {
      "content-length": Buffer.byteLength(body),
      "content-type": "application/json",
    });
    response.end(body);
    return;
  }

  response.writeHead(200, {
    "content-length": archiveSize,
    "content-type": "application/gzip",
  });
  createReadStream(archivePath).pipe(response);
});

server.listen(port, "0.0.0.0", () => {
  process.stdout.write(`mock GitHub listening on ${port}\n`);
});
