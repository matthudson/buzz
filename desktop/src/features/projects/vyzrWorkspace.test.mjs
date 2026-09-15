import assert from "node:assert/strict";
import { test } from "node:test";

import {
  decodeVyzrRecommendation,
  parseVyzrScopes,
  shouldPollVyzrProjection,
  vyzrWorkspaceQueryKey,
} from "./vyzrWorkspace.ts";

test("parses a bounded exact repository scope list", () => {
  assert.deepEqual(
    parseVyzrScopes("docs/guide.md, packages/domain/src\napps/mobile"),
    ["docs/guide.md", "packages/domain/src", "apps/mobile"],
  );
  assert.throws(() => parseVyzrScopes("../outside"), /repository-relative/);
  assert.throws(() => parseVyzrScopes("docs, docs"), /unique/);
  assert.throws(() => parseVyzrScopes(".git/config"), /repository-relative/);
});

test("decodes only a bounded recommendation whose byte count and digest match", async () => {
  const content = JSON.stringify({ verdict: "recommend", holds: ["lead"] });
  const contentBase64 = Buffer.from(content, "utf8").toString("base64");
  const digest = Buffer.from(
    await globalThis.crypto.subtle.digest("SHA-256", Buffer.from(content)),
  ).toString("hex");
  const artifact = {
    digest,
    byteCount: Buffer.byteLength(content),
    contentBase64,
  };
  assert.deepEqual(await decodeVyzrRecommendation(artifact), {
    verdict: "recommend",
    holds: ["lead"],
  });
  await assert.rejects(
    decodeVyzrRecommendation({ ...artifact, byteCount: 1 }),
    /length/,
  );
  await assert.rejects(
    decodeVyzrRecommendation({ ...artifact, digest: "a".repeat(64) }),
    /digest/,
  );
});

test("keys task state by relay, channel, repository, and immutable issue", () => {
  const key = {
    relayOrigin: "https://relay.example",
    channelId: "project-channel",
    repoAddress: "30617:owner:repo",
  };
  assert.deepEqual(vyzrWorkspaceQueryKey(key, "event-id"), [
    "vyzr-task-workspace",
    "https://relay.example",
    "project-channel",
    "30617:owner:repo",
    "event-id",
  ]);
});

test("polling stops after an error or terminal controller state", () => {
  /** @type {any} */
  const projection = { task: { state: "implementing" } };
  assert.equal(shouldPollVyzrProjection(projection, false), true);
  assert.equal(shouldPollVyzrProjection(projection, true), false);
  assert.equal(
    shouldPollVyzrProjection(
      /** @type {any} */ ({ task: { state: "recommended" } }),
      false,
    ),
    false,
  );
});
