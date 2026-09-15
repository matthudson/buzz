import { invokeTauri } from "@/shared/api/tauri";

export type VyzrOperatorOwner = {
  kind: "attended_lead" | "implementation_worker" | "independent_reviewer";
  id: string;
  label: string;
};

export type VyzrOperatorTask = {
  schemaVersion: "development-operator-status.v1";
  taskId: string;
  title: string | null;
  state: string;
  owner: VyzrOperatorOwner;
  nextStep: string;
  blockers: Array<{ code: string }>;
  requiredDecision: string | null;
  lastVerifiedUpdate: {
    sequence: number;
    revision: number;
    kind: string;
    observedAt: string;
  };
  evidencePath: string;
  artifactPath: string | null;
};

export type VyzrControllerEvent = {
  sequence: number;
  revision: number;
  kind: string;
  observed_at: string;
};

export type VyzrRecommendationArtifact = {
  schemaVersion: "development-artifact.v1";
  taskId: string;
  kind: "recommendation";
  byteCount: number;
  digest: string;
  contentBase64: string;
};

export type VyzrTaskWorkspaceProjection = {
  schemaVersion: "buzz-vyzr-task-workspace.v1";
  repoAddress: string;
  relayOrigin: string;
  channelId: string;
  taskId: string;
  requestedWorker: string;
  requestedReviewer: string;
  requestedChecks: string[];
  dataClass: string;
  task: VyzrOperatorTask | null;
  events: VyzrControllerEvent[];
  recommendation: VyzrRecommendationArtifact | null;
};

export type VyzrWorkspaceKey = {
  relayOrigin: string;
  channelId: string;
  repoAddress: string;
};

export function isVyzrWorkspaceAvailable(
  key: VyzrWorkspaceKey,
): Promise<{ configured: boolean }> {
  return invokeTauri("is_vyzr_workspace_available", key);
}

export function getVyzrProjectTask(
  key: VyzrWorkspaceKey,
  issueRepoAddress: string,
  issueId: string,
): Promise<VyzrTaskWorkspaceProjection> {
  return invokeTauri<VyzrTaskWorkspaceProjection>("get_vyzr_project_task", {
    ...key,
    issueRepoAddress,
    issueId,
  });
}

export function submitVyzrProjectTask(input: {
  relayOrigin: string;
  channelId: string;
  repoAddress: string;
  issueRepoAddress: string;
  issueId: string;
  title: string;
  objective: string;
  scopes: string[];
}): Promise<{
  schemaVersion: "development-task-submission-receipt.v1";
  outcome: "accepted_or_replayed";
  taskId: string;
  state: string;
  envelopeDigest: string;
  resourcePlan: {
    maxProviderExecutions: number;
    correctionReserve: number;
    independentReview: "exact_artifact_separate_session";
  };
  executionDriver: "scheduled" | "active" | "terminal";
}> {
  return invokeTauri("submit_vyzr_project_task", { input });
}
