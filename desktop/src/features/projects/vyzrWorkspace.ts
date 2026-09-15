import type { VyzrRecommendationArtifact } from "@/shared/api/tauriVyzrWorkspace";

const SCOPE_PATTERN =
  /^(?!\.git(?:\/|$))(?!\/)(?!.*(?:^|\/)\.\.(?:\/|$))[A-Za-z0-9._ -]+(?:\/[A-Za-z0-9._ -]+)*$/;

export function parseVyzrScopes(value: string): string[] {
  const scopes = value
    .split(/[\n,]/)
    .map((scope) => scope.trim())
    .filter(Boolean);
  if (
    scopes.length === 0 ||
    scopes.length > 64 ||
    new Set(scopes).size !== scopes.length ||
    scopes.some((scope) => scope.length > 256 || !SCOPE_PATTERN.test(scope))
  ) {
    throw new Error(
      "Enter one or more unique repository-relative files or directories.",
    );
  }
  return scopes;
}

export async function decodeVyzrRecommendation(
  artifact: VyzrRecommendationArtifact,
): Promise<unknown> {
  if (!/^[a-f0-9]{64}$/.test(artifact.digest)) {
    throw new Error("The recommendation digest is invalid.");
  }
  let binary: string;
  try {
    binary = globalThis.atob(artifact.contentBase64);
  } catch {
    throw new Error("The recommendation encoding is invalid.");
  }
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  if (bytes.byteLength !== artifact.byteCount || bytes.byteLength > 131_072) {
    throw new Error("The recommendation length does not match its receipt.");
  }
  const digest = Array.from(
    new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", bytes)),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
  if (digest !== artifact.digest) {
    throw new Error("The recommendation digest does not match its content.");
  }
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  return JSON.parse(text) as unknown;
}
