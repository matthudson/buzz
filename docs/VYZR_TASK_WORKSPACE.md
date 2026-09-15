# VYZR task workspace

Buzz can present one repository task's intake, controller progress, decisions,
verified events, and exact recommendation through the existing Projects → Tasks
detail view. VYZR remains the task-transition, admission, execution-budget,
review, and completion authority. Buzz does not add a queue, scheduler, retry
loop, approval engine, result store, or model-generated status summary.

The bridge is dormant unless the host process sets
`BUZZ_VYZR_WORKSPACE_CONFIG` to an absolute local JSON file. Source inclusion
does not install or activate the bridge. The approved runtime must be a clean,
exact VYZR commit whose project envelope authorizes the repository, principal,
scope, checks, data class, worker, reviewer, and shared execution budget.

The configuration has this closed shape:

```json
{
  "schemaVersion": "buzz-vyzr-workspaces.v1",
  "workspaces": [
    {
      "repoAddress": "30617:<owner>:<repository>",
      "projectId": "project-id",
      "principalId": "buzz-desktop",
      "nodeExecutable": "C:\\absolute\\node.exe",
      "nodeSha256": "<sha256>",
      "runtimeRoot": "C:\\absolute\\clean-vyzr-runtime",
      "runtimeRevision": "<full-commit-sha>",
      "runtimeScriptSha256": "<sha256-of-development-mcp.mjs>",
      "repositoryPath": "C:\\absolute\\project-checkout",
      "stateDir": "C:\\absolute\\existing-vyzr-state",
      "envelopePath": "C:\\absolute\\project-envelope.json",
      "envelopeDigest": "<approved-envelope-digest>",
      "codexExecutable": "C:\\absolute\\codex.exe",
      "devinExecutable": "C:\\absolute\\devin.exe",
      "defaultChecks": ["repository"],
      "defaultWorker": "swe-2-direct",
      "defaultReviewer": "codex-sol",
      "dataClass": "INTERNAL"
    }
  ]
}
```

The bridge selects an exact repository mapping, validates and canonicalizes its
executable/runtime paths, binds the envelope's project, principal, and runtime
revision, and launches the pinned VYZR MCP from that approved runtime. The
configuration digest is fixed for the child lifetime; drift requires an app
restart rather than silently changing authority. The UI may request source
scope, but VYZR's server-owned envelope makes the acceptance decision.

If Buzz exits while an admitted execution is active, closing MCP input makes
the existing VYZR driver stop accepting work, wait for its already-admitted
execution to settle, and then close the controller. Buzz deliberately does not
kill that controller subprocess on drop: doing so could strand a provider child
or turn a known execution into an ambiguous effect. The controller remains the
durable source of truth when Buzz is opened again.

Buzz uses the immutable Nostr task event ID as the VYZR submission correlation.
Submission is idempotent, and the resulting VYZR task ID is deterministic. The
task detail polls only while work is active. Recommendation bytes are bounded
and verified against their digest and byte count before display.

This is an attended, same-user integration boundary. It does not establish
hostile same-user containment, automatic deployment, live installation, or
unattended operation. A real installation needs a separate reviewed cutover
with an exact config file, executable provenance, permissions, rollback, and an
end-to-end task whose candidate stops at recommendation.
