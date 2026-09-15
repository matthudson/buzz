import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bot, CircleAlert, Clock3, Route, ShieldCheck } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import type { ProjectIssue, Repository } from "@/features/projects/hooks";
import {
  decodeVyzrRecommendation,
  parseVyzrScopes,
} from "@/features/projects/vyzrWorkspace";
import {
  getVyzrProjectTask,
  submitVyzrProjectTask,
} from "@/shared/api/tauriVyzrWorkspace";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import {
  ProjectDetailMetaList,
  ProjectDetailMetaRow,
} from "./ProjectDetailMeta";
import { ProjectDetailSection } from "./ProjectDetailSection";

const queryKey = (repoAddress: string, issueId: string) => [
  "vyzr-task-workspace",
  repoAddress,
  issueId,
];

function errorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("vyzr_workspace_not_configured")) {
    return "This Buzz installation has no approved VYZR workspace mapping.";
  }
  if (message.includes("vyzr_workspace_mapping_unavailable")) {
    return "This repository is not assigned to an approved VYZR project envelope.";
  }
  return "VYZR status is unavailable. No task transition was inferred.";
}

function Recommendation({
  artifact,
}: {
  artifact: NonNullable<
    Awaited<ReturnType<typeof getVyzrProjectTask>>["recommendation"]
  >;
}) {
  const decoded = useQuery({
    queryKey: ["vyzr-recommendation", artifact.taskId, artifact.digest],
    queryFn: () => decodeVyzrRecommendation(artifact),
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
  });
  return (
    <details className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-xs">
      <summary className="cursor-pointer font-medium">
        Checked recommendation
      </summary>
      <p className="mt-2 break-all text-muted-foreground">
        SHA-256 {artifact.digest}
      </p>
      {decoded.error ? (
        <p className="mt-2 text-destructive">{errorMessage(decoded.error)}</p>
      ) : decoded.isLoading ? (
        <p className="mt-2 text-muted-foreground">Verifying exact bytes…</p>
      ) : (
        <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-background/60 p-3 font-mono text-3xs">
          {JSON.stringify(decoded.data, null, 2)}
        </pre>
      )}
    </details>
  );
}

export function VyzrTaskWorkspacePanel({
  issue,
  project,
}: {
  issue: ProjectIssue;
  project: Repository;
}) {
  const queryClient = useQueryClient();
  const [scopeInput, setScopeInput] = React.useState("");
  const projection = useQuery({
    queryKey: queryKey(project.repoAddress, issue.id),
    queryFn: () => getVyzrProjectTask(project.repoAddress, issue.id),
    refetchInterval: (query) =>
      query.state.data?.task &&
      ![
        "recommended",
        "needs_inspection",
        "cancelled",
        "integration_source_observed",
      ].includes(query.state.data.task.state)
        ? 5_000
        : false,
    retry: false,
  });
  const submit = useMutation({
    mutationFn: async () =>
      submitVyzrProjectTask({
        repoAddress: project.repoAddress,
        issueId: issue.id,
        title: issue.title,
        objective: issue.content.trim() || issue.title,
        scopes: parseVyzrScopes(scopeInput),
      }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: queryKey(project.repoAddress, issue.id),
      });
      toast.success(
        "VYZR accepted the task inside the approved project envelope.",
      );
    },
    onError: (error) => {
      toast.error(
        error instanceof Error
          ? error.message
          : "VYZR did not accept the task.",
      );
    },
  });

  if (projection.isLoading) {
    return (
      <ProjectDetailSection defaultOpen title="VYZR delivery">
        <p className="text-sm text-muted-foreground">
          Loading deterministic controller state…
        </p>
      </ProjectDetailSection>
    );
  }
  if (projection.error || !projection.data) {
    return (
      <ProjectDetailSection defaultOpen title="VYZR delivery">
        <div
          className="flex gap-2 text-sm text-muted-foreground"
          data-testid="vyzr-workspace-unavailable"
        >
          <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
          <span>{errorMessage(projection.error)}</span>
        </div>
      </ProjectDetailSection>
    );
  }
  const data = projection.data;
  if (!data.task) {
    return (
      <ProjectDetailSection defaultOpen title="VYZR delivery">
        <div className="space-y-3" data-testid="vyzr-workspace-intake">
          <p className="text-sm text-muted-foreground">
            Start this task through the existing controller. The approved
            project envelope—not this form—sets the worker, reviewer, checks,
            permissions, and execution budget.
          </p>
          <div className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-xs text-muted-foreground">
            Requested route: {data.requestedWorker} → {data.requestedReviewer} ·{" "}
            {data.requestedChecks.join(", ")} · {data.dataClass}
          </div>
          <label
            className="block space-y-1.5 text-sm font-medium"
            htmlFor="vyzr-workspace-scopes"
          >
            <span>Allowed source paths</span>
            <Input
              data-testid="vyzr-workspace-scopes"
              disabled={submit.isPending}
              id="vyzr-workspace-scopes"
              onChange={(event) => setScopeInput(event.target.value)}
              placeholder="docs/guide.md, packages/domain/src"
              value={scopeInput}
            />
          </label>
          <Button
            data-testid="vyzr-workspace-submit"
            disabled={submit.isPending || scopeInput.trim().length === 0}
            onClick={() => submit.mutate()}
            type="button"
          >
            {submit.isPending ? "Submitting…" : "Start with VYZR"}
          </Button>
        </div>
      </ProjectDetailSection>
    );
  }

  const task = data.task;
  return (
    <ProjectDetailSection defaultOpen title="VYZR delivery">
      <div className="space-y-3" data-testid="vyzr-workspace-status">
        <ProjectDetailMetaList>
          <ProjectDetailMetaRow icon={ShieldCheck} label="Controller state">
            {task.state}
          </ProjectDetailMetaRow>
          <ProjectDetailMetaRow icon={Bot} label="Owner">
            {task.owner.label}
          </ProjectDetailMetaRow>
          <ProjectDetailMetaRow icon={Route} label="Requested route">
            {data.requestedWorker} → {data.requestedReviewer}
          </ProjectDetailMetaRow>
          <ProjectDetailMetaRow icon={Clock3} label="Last verified">
            <span
              title={new Date(
                task.lastVerifiedUpdate.observedAt,
              ).toLocaleString()}
            >
              {task.lastVerifiedUpdate.kind} ·{" "}
              {new Date(task.lastVerifiedUpdate.observedAt).toLocaleString()}
            </span>
          </ProjectDetailMetaRow>
        </ProjectDetailMetaList>
        <div className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-sm">
          <p className="font-medium">Next</p>
          <p className="mt-1 text-muted-foreground">{task.nextStep}</p>
        </div>
        {task.blockers.length > 0 || task.requiredDecision ? (
          <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-sm">
            <p className="font-medium">Blocked / decision</p>
            <p className="mt-1 text-muted-foreground">
              {task.blockers.map((blocker) => blocker.code).join(", ") ||
                "No blocker"}
              {task.requiredDecision ? ` · ${task.requiredDecision}` : ""}
            </p>
          </div>
        ) : null}
        {data.events.length > 0 ? (
          <details className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-xs">
            <summary className="cursor-pointer font-medium">
              Verified progress ({data.events.length})
            </summary>
            <ol className="mt-2 space-y-1 text-muted-foreground">
              {data.events.slice(-8).map((event) => (
                <li key={event.sequence}>
                  {event.sequence}. {event.kind} ·{" "}
                  {new Date(event.observed_at).toLocaleString()}
                </li>
              ))}
            </ol>
          </details>
        ) : null}
        {data.recommendation ? (
          <Recommendation artifact={data.recommendation} />
        ) : null}
      </div>
    </ProjectDetailSection>
  );
}
