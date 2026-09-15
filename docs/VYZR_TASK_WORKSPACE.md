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
      "relayOrigin": "https://exact-community-relay.example",
      "channelId": "exact-project-channel-id",
      "repoAddress": "30617:<owner>:<repository>",
      "projectId": "project-id",
      "principalId": "buzz-desktop",
      "nodeExecutable": "C:\\absolute\\node.exe",
      "nodeSha256": "<sha256>",
      "runtimeRoot": "C:\\absolute\\clean-vyzr-runtime",
      "runtimeRevision": "<full-commit-sha>",
      "runtimeScriptSha256": "<sha256-of-development-mcp.mjs>",
      "runtimeTreeSha256": "<bounded-tree-digest-excluding-.git>",
      "repositoryPath": "C:\\absolute\\project-checkout",
      "stateDir": "C:\\absolute\\existing-vyzr-state",
      "envelopePath": "C:\\absolute\\project-envelope.json",
      "envelopeDigest": "<approved-envelope-digest>",
      "codexExecutable": "C:\\absolute\\codex.exe",
      "codexSha256": "<sha256>",
      "devinExecutable": "C:\\absolute\\devin.exe",
      "devinSha256": "<sha256>",
      "defaultChecks": ["repository"],
      "defaultWorker": "swe-2-direct",
      "defaultReviewer": "codex-sol",
      "dataClass": "INTERNAL"
    }
  ]
}
```

The bridge selects an exact relay, channel, and repository mapping, validates
and canonicalizes its executable/runtime paths, binds the canonical envelope
digest and the envelope's project, principal, and runtime revision, and
launches the pinned VYZR MCP from that approved runtime. The bounded runtime
tree digest covers every ordinary file below the runtime root except `.git`;
symlinks, special files, more than 10,000 total entries, depth beyond 64, or
more than 512 MiB fail closed. Files are sorted by relative path and hashed as repeated UTF-8
forward-slash path, NUL, decimal byte length, NUL, and exact file bytes. Node,
Codex, and Devin are separately bound by SHA-256. Validation is
repeated immediately before process creation. The configuration digest is
fixed for the child lifetime; drift requires an app restart rather than
silently changing authority. The UI may request source scope, but VYZR's
server-owned envelope makes the acceptance decision.

Full runtime, envelope (bounded to 256 KiB), and provider validation occurs
when a controller client is created and again at the final practical point
before its process is spawned. Active status polling reads only the bounded
configuration bytes and compares their digest with the client binding; it does
not synchronously rehash the complete runtime tree every five seconds.

If Buzz exits while an admitted execution is active, closing MCP input makes
the existing VYZR driver stop accepting work, wait for its already-admitted
execution to settle, and then close the controller. Buzz deliberately does not
kill that controller subprocess on drop: doing so could strand a provider child
or turn a known execution into an ambiguous effect. The controller remains the
durable source of truth when Buzz is opened again.

Buzz binds the immutable Nostr task event ID to its exact relay, project
channel, and repository coordinate before deriving VYZR submission
correlation. A task's explicit `h` tag is used byte-for-byte and must pass the
backend's exact identifier validation; it is never trimmed or normalized.
Native Buzz tasks,
which do not carry that tag, use the selected repository's signed channel
binding. Submission is idempotent, and the resulting VYZR task ID is
deterministic. Tool errors and non-exact admission receipts are rejected,
including a resource plan that differs from the approved envelope. The shared
provider-execution ceiling, correction reserve, and independent-review rule are
shown in the task workspace.
Transport or framing failures evict the affected cached client; fixed polling
stops on an error instead of consuming an unbounded retry loop. The task detail
polls only while work is active. Event pages must be strictly ordered and end
at the exact task projection checkpoint; concurrent drift fails closed and is
retried only by a later ordinary UI refresh. Recommendation bytes are bounded and verified
against their digest and byte count before display. When no bridge config is
present, the task workspace renders nothing.

This is an attended, same-user integration boundary. It does not establish
hostile same-user containment, automatic deployment, live installation, or
unattended operation. A real installation needs a separate reviewed cutover
with an exact config file, executable provenance, permissions, rollback, and an
end-to-end task whose candidate stops at recommendation.
