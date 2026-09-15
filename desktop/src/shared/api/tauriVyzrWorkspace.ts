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
  taskId: string;
  requestedWorker: string;
  requestedReviewer: string;
  requestedChecks: string[];
  dataClass: string;
  task: VyzrOperatorTask | null;
  events: VyzrControllerEvent[];
  recommendation: VyzrRecommendationArtifact | null;
};

export function getVyzrProjectTask(
  repoAddress: string,
  issueId: string,
): Promise<VyzrTaskWorkspaceProjection> {
  return invokeTauri<VyzrTaskWorkspaceProjection>("get_vyzr_project_task", {
    repoAddress,
    issueId,
  });
}

export function submitVyzrProjectTask(input: {
  repoAddress: string;
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
  executionDriver: "scheduled" | "active" | "terminal";
}> {
  return invokeTauri("submit_vyzr_project_task", { input });
}
