//! Narrow local bridge from Buzz project tasks to the existing VYZR controller.
//!
//! VYZR remains the only task-transition authority. Buzz selects one approved
//! project mapping, submits an idempotent task, and renders bounded projections.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use tauri::State;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

#[path = "vyzr_workspace_integrity.rs"]
mod integrity;
#[cfg(test)]
use integrity::{collect_runtime_files, count_runtime_entry};
use integrity::{read_bounded_file, runtime_tree_digest};

const CONFIG_ENV: &str = "BUZZ_VYZR_WORKSPACE_CONFIG";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_ENVELOPE_BYTES: u64 = 256 * 1024;
const MAX_RPC_BYTES: usize = 512 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RUNTIME_FILES: usize = 10_000;
const MAX_RUNTIME_ENTRIES: usize = 10_000;
const MAX_RUNTIME_DEPTH: usize = 64;
const MAX_RUNTIME_BYTES: u64 = 512 * 1024 * 1024;

/// Process-local cache of validated VYZR MCP connections.
#[derive(Default)]
pub struct VyzrWorkspaceState {
    clients: Mutex<HashMap<String, VyzrMcpClient>>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceConfigFile {
    schema_version: String,
    workspaces: Vec<WorkspaceConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceConfig {
    relay_origin: String,
    channel_id: String,
    repo_address: String,
    project_id: String,
    principal_id: String,
    node_executable: String,
    node_sha256: String,
    runtime_root: String,
    runtime_revision: String,
    runtime_script_sha256: String,
    runtime_tree_sha256: String,
    repository_path: String,
    state_dir: String,
    envelope_path: String,
    envelope_digest: String,
    codex_executable: String,
    codex_sha256: String,
    devin_executable: String,
    devin_sha256: String,
    default_checks: Vec<String>,
    default_worker: String,
    default_reviewer: String,
    data_class: String,
    #[serde(skip)]
    resource_plan: Option<ResourcePlan>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectEnvelope {
    schema_version: String,
    project_id: String,
    principal_id: String,
    revision: String,
    repository_digest: String,
    store_identity_digest: String,
    expires_at: String,
    allowed_scopes: Vec<String>,
    allowed_checks: Vec<String>,
    allowed_bindings: Vec<String>,
    data_classes: Vec<String>,
    resource_plan: ResourcePlan,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResourcePlan {
    max_provider_executions: u8,
    correction_reserve: u8,
    independent_review: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperatorOwner {
    kind: String,
    id: String,
    label: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OperatorBlocker {
    code: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LastVerifiedUpdate {
    sequence: u64,
    revision: u64,
    kind: String,
    observed_at: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperatorTask {
    schema_version: String,
    task_id: String,
    title: Option<String>,
    state: String,
    owner: OperatorOwner,
    next_step: String,
    blockers: Vec<OperatorBlocker>,
    required_decision: Option<String>,
    last_verified_update: LastVerifiedUpdate,
    evidence_path: String,
    artifact_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperatorView {
    schema_version: String,
    tasks: Vec<OperatorTask>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControllerEvent {
    sequence: u64,
    revision: u64,
    kind: String,
    observed_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventPage {
    schema_version: String,
    task_id: String,
    events: Vec<ControllerEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolTextContent {
    r#type: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolCallResult {
    content: Vec<ToolTextContent>,
    structured_content: Value,
    is_error: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VyzrWorkspaceAvailability {
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VyzrWorkspaceProjection {
    schema_version: &'static str,
    repo_address: String,
    relay_origin: String,
    channel_id: String,
    task_id: String,
    requested_worker: String,
    requested_reviewer: String,
    requested_checks: Vec<String>,
    data_class: String,
    resource_plan: ResourcePlan,
    task: Option<OperatorTask>,
    events: Vec<ControllerEvent>,
    recommendation: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitVyzrProjectTaskInput {
    relay_origin: String,
    channel_id: String,
    repo_address: String,
    issue_repo_address: String,
    issue_id: String,
    title: String,
    objective: String,
    scopes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionReceipt {
    schema_version: String,
    outcome: String,
    task_id: String,
    state: String,
    envelope_digest: String,
    resource_plan: ResourcePlan,
    execution_driver: String,
}

type McpReader = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
type McpWriter = Box<dyn AsyncWrite + Send + Unpin>;

struct VyzrMcpClient {
    _child: Child,
    transport: McpTransport,
    config: WorkspaceConfig,
}

struct McpTransport {
    stdin: McpWriter,
    stdout: McpReader,
    next_id: u64,
    config_digest: String,
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|_| "vyzr_workspace_file_unavailable".to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "vyzr_workspace_file_unavailable".to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn safe_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn safe_line(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{0085}' | '\u{2028}' | '\u{2029}'))
}

fn absolute_existing(path: &str, directory: bool) -> Result<PathBuf, String> {
    let supplied = PathBuf::from(path);
    if !supplied.is_absolute() {
        return Err("vyzr_workspace_path_not_absolute".to_string());
    }
    let metadata = std::fs::symlink_metadata(&supplied)
        .map_err(|_| "vyzr_workspace_path_unavailable".to_string())?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err("vyzr_workspace_path_unsafe".to_string());
    }
    supplied
        .canonicalize()
        .map_err(|_| "vyzr_workspace_path_unavailable".to_string())
}

fn validate_workspace(mut config: WorkspaceConfig) -> Result<WorkspaceConfig, String> {
    let unique_checks = config
        .default_checks
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    if !safe_line(&config.relay_origin, 2048)
        || !safe_identifier(&config.channel_id, 128)
        || !safe_line(&config.repo_address, 1024)
        || !safe_identifier(&config.project_id, 128)
        || !safe_identifier(&config.principal_id, 101)
        || !is_hex(&config.node_sha256, 64)
        || !is_hex(&config.runtime_revision, 40)
        || !is_hex(&config.runtime_script_sha256, 64)
        || !is_hex(&config.runtime_tree_sha256, 64)
        || !is_hex(&config.envelope_digest, 64)
        || !is_hex(&config.codex_sha256, 64)
        || !is_hex(&config.devin_sha256, 64)
        || !matches!(
            config.default_worker.as_str(),
            "codex-spark" | "codex-sol" | "swe-2" | "swe-2-direct"
        )
        || !matches!(
            config.default_reviewer.as_str(),
            "codex-spark" | "codex-sol" | "swe-2" | "swe-2-direct"
        )
        || !matches!(config.data_class.as_str(), "PUBLIC_SYNTHETIC" | "INTERNAL")
        || config.default_checks.is_empty()
        || config.default_checks.len() > 4
        || unique_checks != config.default_checks.len()
        || config.default_checks.iter().any(|check| {
            !matches!(
                check.as_str(),
                "docs" | "operators" | "orchestrator" | "repository"
            )
        })
    {
        return Err("vyzr_workspace_config_invalid".to_string());
    }
    let node = absolute_existing(&config.node_executable, false)?;
    if sha256_file(&node)? != config.node_sha256 {
        return Err("vyzr_workspace_node_integrity_mismatch".to_string());
    }
    let runtime = absolute_existing(&config.runtime_root, true)?;
    let script = absolute_existing(
        runtime
            .join("scripts/orchestrator/development-mcp.mjs")
            .to_string_lossy()
            .as_ref(),
        false,
    )?;
    if sha256_file(&script)? != config.runtime_script_sha256 {
        return Err("vyzr_workspace_runtime_integrity_mismatch".to_string());
    }
    if runtime_tree_digest(&runtime)? != config.runtime_tree_sha256 {
        return Err("vyzr_workspace_runtime_integrity_mismatch".to_string());
    }
    let repository = absolute_existing(&config.repository_path, true)?;
    let state = absolute_existing(&config.state_dir, true)?;
    let envelope = absolute_existing(&config.envelope_path, false)?;
    let codex = absolute_existing(&config.codex_executable, false)?;
    let devin = absolute_existing(&config.devin_executable, false)?;
    if sha256_file(&codex)? != config.codex_sha256 || sha256_file(&devin)? != config.devin_sha256 {
        return Err("vyzr_workspace_provider_integrity_mismatch".to_string());
    }
    let envelope_bytes = read_bounded_file(
        &envelope,
        MAX_ENVELOPE_BYTES,
        "vyzr_workspace_envelope_unavailable",
    )?;
    let envelope_value: ProjectEnvelope = serde_json::from_slice(&envelope_bytes)
        .map_err(|_| "vyzr_workspace_envelope_invalid".to_string())?;
    let canonical_envelope = serde_json::to_vec(&envelope_value)
        .map_err(|_| "vyzr_workspace_envelope_invalid".to_string())?;
    if sha256_bytes(&canonical_envelope) != config.envelope_digest
        || envelope_value.schema_version != "development-project-envelope.v1"
        || envelope_value.project_id != config.project_id
        || envelope_value.principal_id != config.principal_id
        || envelope_value.revision != config.runtime_revision
    {
        return Err("vyzr_workspace_envelope_binding_mismatch".to_string());
    }
    config.resource_plan = Some(envelope_value.resource_plan);
    config.node_executable = node.to_string_lossy().into_owned();
    config.runtime_root = runtime.to_string_lossy().into_owned();
    config.repository_path = repository.to_string_lossy().into_owned();
    config.state_dir = state.to_string_lossy().into_owned();
    config.envelope_path = envelope.to_string_lossy().into_owned();
    config.codex_executable = codex.to_string_lossy().into_owned();
    config.devin_executable = devin.to_string_lossy().into_owned();
    Ok(config)
}

fn load_workspace_config(
    relay_origin: &str,
    channel_id: &str,
    repo_address: &str,
) -> Result<(WorkspaceConfig, String), String> {
    if !safe_line(relay_origin, 2048)
        || !safe_identifier(channel_id, 128)
        || !safe_line(repo_address, 1024)
    {
        return Err("vyzr_workspace_repo_address_invalid".to_string());
    }
    let path =
        std::env::var_os(CONFIG_ENV).ok_or_else(|| "vyzr_workspace_not_configured".to_string())?;
    let path = absolute_existing(PathBuf::from(path).to_string_lossy().as_ref(), false)?;
    let metadata =
        std::fs::metadata(&path).map_err(|_| "vyzr_workspace_config_unavailable".to_string())?;
    if metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return Err("vyzr_workspace_config_unsafe".to_string());
    }
    let bytes =
        std::fs::read(&path).map_err(|_| "vyzr_workspace_config_unavailable".to_string())?;
    if bytes.len() as u64 != metadata.len() {
        return Err("vyzr_workspace_config_changed".to_string());
    }
    let parsed: WorkspaceConfigFile =
        serde_json::from_slice(&bytes).map_err(|_| "vyzr_workspace_config_invalid".to_string())?;
    if parsed.schema_version != "buzz-vyzr-workspaces.v1"
        || parsed.workspaces.is_empty()
        || parsed.workspaces.len() > 32
    {
        return Err("vyzr_workspace_config_invalid".to_string());
    }
    let matches: Vec<_> = parsed
        .workspaces
        .into_iter()
        .filter(|workspace| {
            workspace.relay_origin == relay_origin
                && workspace.channel_id == channel_id
                && workspace.repo_address == repo_address
        })
        .collect();
    if matches.len() != 1 {
        return Err("vyzr_workspace_mapping_unavailable".to_string());
    }
    let workspace = matches
        .into_iter()
        .next()
        .ok_or_else(|| "vyzr_workspace_mapping_unavailable".to_string())?;
    Ok((workspace, sha256_bytes(&bytes)))
}

fn load_validated_workspace(
    relay_origin: &str,
    channel_id: &str,
    repo_address: &str,
) -> Result<(WorkspaceConfig, String), String> {
    let (workspace, digest) = load_workspace_config(relay_origin, channel_id, repo_address)?;
    Ok((validate_workspace(workspace)?, digest))
}

fn issue_submission_id(
    relay_origin: &str,
    channel_id: &str,
    repo_address: &str,
    issue_id: &str,
) -> Result<String, String> {
    if !is_hex(issue_id, 64) {
        return Err("vyzr_workspace_issue_id_invalid".to_string());
    }
    let digest = sha256_bytes(
        format!("{relay_origin}\n{channel_id}\n{repo_address}\n{issue_id}").as_bytes(),
    );
    Ok(format!("buzz-{digest}"))
}

fn task_id(config: &WorkspaceConfig, issue_id: &str) -> Result<String, String> {
    let submission = issue_submission_id(
        &config.relay_origin,
        &config.channel_id,
        &config.repo_address,
        issue_id,
    )?;
    let input = format!(
        "{}\n{}\n{}",
        config.principal_id, config.project_id, submission
    );
    Ok(format!("dev-{}", sha256_bytes(input.as_bytes())))
}

fn validate_recommendation_artifact(value: Value, expected_task_id: &str) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "vyzr_workspace_artifact_invalid".to_string())?;
    let expected_keys = [
        "schemaVersion",
        "taskId",
        "kind",
        "byteCount",
        "digest",
        "contentBase64",
    ];
    if object.len() != expected_keys.len()
        || expected_keys.iter().any(|key| !object.contains_key(*key))
        || value.get("schemaVersion").and_then(Value::as_str) != Some("development-artifact.v1")
        || value.get("taskId").and_then(Value::as_str) != Some(expected_task_id)
        || value.get("kind").and_then(Value::as_str) != Some("recommendation")
    {
        return Err("vyzr_workspace_artifact_invalid".to_string());
    }
    let byte_count = value
        .get("byteCount")
        .and_then(Value::as_u64)
        .filter(|count| *count <= 131_072)
        .ok_or_else(|| "vyzr_workspace_artifact_invalid".to_string())?;
    let digest = value
        .get("digest")
        .and_then(Value::as_str)
        .filter(|digest| is_hex(digest, 64))
        .ok_or_else(|| "vyzr_workspace_artifact_invalid".to_string())?;
    let encoded = value
        .get("contentBase64")
        .and_then(Value::as_str)
        .filter(|encoded| encoded.len() <= 174_764)
        .ok_or_else(|| "vyzr_workspace_artifact_invalid".to_string())?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| "vyzr_workspace_artifact_invalid".to_string())?;
    if bytes.len() as u64 != byte_count || sha256_bytes(&bytes) != digest {
        return Err("vyzr_workspace_artifact_mismatch".to_string());
    }
    Ok(value)
}

fn valid_stage(value: &str) -> bool {
    matches!(
        value,
        "starting"
            | "implementing"
            | "correcting"
            | "reviewing"
            | "reviewed"
            | "rehearsed"
            | "recommended"
            | "needs_inspection"
            | "integration_source_observed"
            | "cancelled"
    )
}

fn validate_operator_task(task: &OperatorTask) -> Result<(), String> {
    let task_digest = task.task_id.strip_prefix("dev-");
    if task.schema_version != "development-operator-status.v1"
        || !task_digest.is_some_and(|value| is_hex(value, 64))
        || !valid_stage(&task.state)
        || !matches!(
            task.owner.kind.as_str(),
            "attended_lead" | "implementation_worker" | "independent_reviewer"
        )
        || !safe_identifier(&task.owner.id, 128)
        || !safe_line(&task.owner.label, 256)
        || !safe_line(&task.next_step, 1024)
        || task
            .title
            .as_ref()
            .is_some_and(|value| !safe_line(value, 256))
        || task.blockers.len() > 64
        || task
            .blockers
            .iter()
            .any(|blocker| !safe_identifier(&blocker.code, 128))
        || task
            .required_decision
            .as_ref()
            .is_some_and(|value| !safe_identifier(value, 128))
        || task.last_verified_update.sequence == 0
        || task.last_verified_update.revision == 0
        || !safe_identifier(&task.last_verified_update.kind, 128)
        || !safe_line(&task.last_verified_update.observed_at, 64)
        || !safe_line(&task.evidence_path, 4096)
        || task
            .artifact_path
            .as_ref()
            .is_some_and(|value| !safe_line(value, 4096))
    {
        return Err("vyzr_workspace_projection_invalid".to_string());
    }
    Ok(())
}

fn validate_controller_event(event: &ControllerEvent) -> Result<(), String> {
    if event.sequence == 0
        || event.revision == 0
        || !safe_identifier(&event.kind, 128)
        || !safe_line(&event.observed_at, 64)
    {
        return Err("vyzr_workspace_events_invalid".to_string());
    }
    Ok(())
}

fn validate_event_snapshot(
    events: &[ControllerEvent],
    last_verified: &LastVerifiedUpdate,
) -> Result<(), String> {
    let mut prior_sequence = 0;
    for event in events {
        validate_controller_event(event)?;
        if event.sequence <= prior_sequence || event.sequence > last_verified.sequence {
            return Err("vyzr_workspace_events_snapshot_mismatch".to_string());
        }
        prior_sequence = event.sequence;
    }
    let final_event = events
        .last()
        .ok_or_else(|| "vyzr_workspace_events_snapshot_mismatch".to_string())?;
    if final_event.sequence != last_verified.sequence
        || final_event.revision != last_verified.revision
        || final_event.kind != last_verified.kind
        || final_event.observed_at != last_verified.observed_at
    {
        return Err("vyzr_workspace_events_snapshot_mismatch".to_string());
    }
    Ok(())
}

async fn read_bounded_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| "vyzr_workspace_transport_failed".to_string())?;
        if available.is_empty() {
            return Err("vyzr_workspace_transport_closed".to_string());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if result.len() + count > MAX_RPC_BYTES {
            return Err("vyzr_workspace_response_too_large".to_string());
        }
        result.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            result.pop();
            if result.last() == Some(&b'\r') {
                result.pop();
            }
            return Ok(result);
        }
    }
}

impl VyzrMcpClient {
    async fn launch(config: &WorkspaceConfig, config_digest: String) -> Result<Self, String> {
        let mut command = build_mcp_command(config)?;
        let mut child = command
            .spawn()
            .map_err(|_| "vyzr_workspace_spawn_failed".to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "vyzr_workspace_spawn_failed".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "vyzr_workspace_spawn_failed".to_string())?;
        let mut client = Self {
            _child: child,
            transport: McpTransport {
                stdin: Box::new(stdin),
                stdout: BufReader::new(Box::new(stdout)),
                next_id: 1,
                config_digest,
            },
            config: config.clone(),
        };
        let initialized = client
            .transport.request("initialize", json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "buzz-task-workspace", "version": env!("CARGO_PKG_VERSION") }
            }))
            .await?;
        if initialized.pointer("/result/protocolVersion")
            != Some(&Value::String("2025-06-18".to_string()))
        {
            return Err("vyzr_workspace_protocol_mismatch".to_string());
        }
        client
            .transport
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }
}

impl McpTransport {
    async fn write_value(&mut self, value: &Value) -> Result<(), String> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| "vyzr_workspace_request_invalid".to_string())?;
        if bytes.len() > 256 * 1024 {
            return Err("vyzr_workspace_request_too_large".to_string());
        }
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|_| "vyzr_workspace_transport_failed".to_string())?;
        self.stdin
            .flush()
            .await
            .map_err(|_| "vyzr_workspace_transport_failed".to_string())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        timeout(
            RPC_TIMEOUT,
            self.write_value(&json!({ "jsonrpc": "2.0", "method": method, "params": params })),
        )
        .await
        .map_err(|_| "vyzr_workspace_transport_timeout".to_string())?
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        timeout(RPC_TIMEOUT, self.request_inner(method, params))
            .await
            .map_err(|_| "vyzr_workspace_transport_timeout".to_string())?
    }

    async fn request_inner(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "vyzr_workspace_request_id_exhausted".to_string())?;
        self.write_value(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await?;
        let bytes = read_bounded_line(&mut self.stdout).await?;
        let response: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "vyzr_workspace_response_invalid".to_string())?;
        if response.get("jsonrpc") != Some(&Value::String("2.0".to_string()))
            || response.get("id") != Some(&Value::Number(id.into()))
        {
            return Err("vyzr_workspace_response_mismatch".to_string());
        }
        if let Some(message) = response.pointer("/error/message").and_then(Value::as_str) {
            return Err(format!("vyzr_workspace_controller:{message}"));
        }
        if response.get("result").is_none() {
            return Err("vyzr_workspace_response_invalid".to_string());
        }
        Ok(response)
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let response = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let result: ToolCallResult = serde_json::from_value(
            response
                .get("result")
                .cloned()
                .ok_or_else(|| "vyzr_workspace_tool_result_invalid".to_string())?,
        )
        .map_err(|_| "vyzr_workspace_tool_result_invalid".to_string())?;
        if result.is_error {
            return Err("vyzr_workspace_tool_failed".to_string());
        }
        if result.content.len() != 1
            || result.content[0].r#type != "text"
            || result.content[0].text.len() > MAX_RPC_BYTES
            || serde_json::from_str::<Value>(&result.content[0].text).ok()
                != Some(result.structured_content.clone())
        {
            return Err("vyzr_workspace_tool_result_invalid".to_string());
        }
        Ok(result.structured_content)
    }
}

fn build_mcp_command(config: &WorkspaceConfig) -> Result<Command, String> {
    let config = validate_workspace(config.clone())?;
    build_validated_mcp_command(&config)
}

fn build_validated_mcp_command(config: &WorkspaceConfig) -> Result<Command, String> {
    let runtime = absolute_existing(&config.runtime_root, true)?;
    let script = runtime.join("scripts/orchestrator/development-mcp.mjs");
    let mut command = Command::new(&config.node_executable);
    command
        .arg(script)
        .args([
            "--repo",
            &config.repository_path,
            "--state-dir",
            &config.state_dir,
            "--envelope",
            &config.envelope_path,
            "--envelope-digest",
            &config.envelope_digest,
            "--principal",
            &config.principal_id,
            "--execution-driver",
            "attended_local",
            "--codex-exe",
            &config.codex_executable,
            "--devin-exe",
            &config.devin_executable,
        ])
        .current_dir(runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    Ok(command)
}

async fn with_client<T, F>(
    state: &VyzrWorkspaceState,
    relay_origin: &str,
    channel_id: &str,
    repo_address: &str,
    operation: F,
) -> Result<T, String>
where
    T: Send,
    F: for<'a> FnOnce(
        &'a mut VyzrMcpClient,
        WorkspaceConfig,
    ) -> Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>,
{
    let (unvalidated_config, config_digest) =
        load_workspace_config(relay_origin, channel_id, repo_address)?;
    let client_key = format!("{relay_origin}\n{channel_id}\n{repo_address}");
    let mut clients = state.clients.lock().await;
    let operation_config = if let Some(existing) = clients.get(&client_key) {
        cached_operation_config(
            &existing.transport.config_digest,
            &config_digest,
            &existing.config,
        )?
    } else {
        let config = validate_workspace(unvalidated_config)?;
        let client = VyzrMcpClient::launch(&config, config_digest).await?;
        clients.insert(client_key.clone(), client);
        config
    };
    let result = match clients.get_mut(&client_key) {
        Some(client) => operation(client, operation_config).await,
        None => Err("vyzr_workspace_client_unavailable".to_string()),
    };
    if result.is_err() {
        clients.remove(&client_key);
    }
    result
}

fn cached_operation_config(
    cached_digest: &str,
    current_digest: &str,
    cached_config: &WorkspaceConfig,
) -> Result<WorkspaceConfig, String> {
    if cached_digest != current_digest {
        return Err("vyzr_workspace_configuration_changed_restart_required".to_string());
    }
    Ok(cached_config.clone())
}

/// Reports whether an exact relay/channel/repository mapping is configured.
#[tauri::command]
pub async fn is_vyzr_workspace_available(
    relay_origin: String,
    channel_id: String,
    repo_address: String,
) -> Result<VyzrWorkspaceAvailability, String> {
    if std::env::var_os(CONFIG_ENV).is_none() {
        return Ok(VyzrWorkspaceAvailability { configured: false });
    }
    load_validated_workspace(&relay_origin, &channel_id, &repo_address)?;
    Ok(VyzrWorkspaceAvailability { configured: true })
}

/// Reads one exact task projection from the configured VYZR controller.
#[tauri::command]
pub async fn get_vyzr_project_task(
    relay_origin: String,
    channel_id: String,
    repo_address: String,
    issue_repo_address: String,
    issue_id: String,
    state: State<'_, VyzrWorkspaceState>,
) -> Result<VyzrWorkspaceProjection, String> {
    if issue_repo_address != repo_address {
        return Err("vyzr_workspace_issue_repository_mismatch".to_string());
    }
    with_client(
        &state,
        &relay_origin,
        &channel_id,
        &repo_address,
        |client, config| {
            Box::pin(async move {
                let expected_task_id = task_id(&config, &issue_id)?;
                let view: OperatorView = serde_json::from_value(
                    client
                        .transport
                        .tool("development_tasks", json!({}))
                        .await?,
                )
                .map_err(|_| "vyzr_workspace_projection_invalid".to_string())?;
                if view.schema_version != "development-operator-view.v1" {
                    return Err("vyzr_workspace_projection_invalid".to_string());
                }
                for task in &view.tasks {
                    validate_operator_task(task)?;
                }
                let task = view
                    .tasks
                    .iter()
                    .find(|item| item.task_id == expected_task_id)
                    .cloned();
                let mut events = Vec::new();
                let mut recommendation = None;
                if let Some(ref current) = task {
                    let last_sequence = current.last_verified_update.sequence;
                    let page: EventPage = serde_json::from_value(
                        client
                            .transport
                            .tool(
                                "development_events",
                                json!({
                                    "taskId": expected_task_id,
                                    "afterSequence": last_sequence.saturating_sub(8),
                                    "limit": 8
                                }),
                            )
                            .await?,
                    )
                    .map_err(|_| "vyzr_workspace_events_invalid".to_string())?;
                    if page.schema_version != "development-event-page.v1"
                        || page.task_id != expected_task_id
                        || page.events.len() > 8
                    {
                        return Err("vyzr_workspace_events_invalid".to_string());
                    }
                    validate_event_snapshot(&page.events, &current.last_verified_update)?;
                    events = page.events;
                    if current.state == "recommended" {
                        let artifact = client
                            .transport
                            .tool(
                                "development_artifact",
                                json!({ "taskId": expected_task_id, "kind": "recommendation" }),
                            )
                            .await?;
                        recommendation = Some(validate_recommendation_artifact(
                            artifact,
                            &expected_task_id,
                        )?);
                    }
                }
                Ok(VyzrWorkspaceProjection {
                    schema_version: "buzz-vyzr-task-workspace.v1",
                    repo_address: config.repo_address,
                    relay_origin: config.relay_origin,
                    channel_id: config.channel_id,
                    task_id: expected_task_id,
                    requested_worker: config.default_worker,
                    requested_reviewer: config.default_reviewer,
                    requested_checks: config.default_checks,
                    data_class: config.data_class,
                    resource_plan: config
                        .resource_plan
                        .ok_or_else(|| "vyzr_workspace_envelope_binding_mismatch".to_string())?,
                    task,
                    events,
                    recommendation,
                })
            })
        },
    )
    .await
}

fn validate_submission_receipt(
    value: Value,
    expected_task_id: &str,
    expected_envelope_digest: &str,
    expected_resource_plan: &ResourcePlan,
) -> Result<SubmissionReceipt, String> {
    let receipt: SubmissionReceipt = serde_json::from_value(value)
        .map_err(|_| "vyzr_workspace_submission_receipt_invalid".to_string())?;
    if receipt.schema_version != "development-task-submission-receipt.v1"
        || receipt.outcome != "accepted_or_replayed"
        || receipt.task_id != expected_task_id
        || receipt.envelope_digest != expected_envelope_digest
        || !matches!(
            receipt.execution_driver.as_str(),
            "scheduled" | "active" | "terminal"
        )
        || !matches!(
            receipt.state.as_str(),
            "starting"
                | "implementing"
                | "reviewing"
                | "correcting"
                | "reviewed"
                | "rehearsed"
                | "recommended"
                | "needs_inspection"
                | "integration_source_observed"
                | "cancelled"
        )
        || receipt.resource_plan != *expected_resource_plan
    {
        return Err("vyzr_workspace_submission_receipt_invalid".to_string());
    }
    Ok(receipt)
}

/// Submits one idempotent task request to the configured VYZR controller.
#[tauri::command]
pub async fn submit_vyzr_project_task(
    input: SubmitVyzrProjectTaskInput,
    state: State<'_, VyzrWorkspaceState>,
) -> Result<SubmissionReceipt, String> {
    if !safe_line(&input.title, 256)
        || input.objective.len() > 16_000
        || input.objective.chars().any(|ch| ch == '\0')
        || input.scopes.is_empty()
        || input.scopes.len() > 64
        || input.scopes.iter().any(|scope| !safe_line(scope, 256))
    {
        return Err("vyzr_workspace_submission_invalid".to_string());
    }
    if input.issue_repo_address != input.repo_address {
        return Err("vyzr_workspace_issue_repository_mismatch".to_string());
    }
    let repo_address = input.repo_address.clone();
    let relay_origin = input.relay_origin.clone();
    let channel_id = input.channel_id.clone();
    with_client(
        &state,
        &relay_origin,
        &channel_id,
        &repo_address,
        |client, config| {
            Box::pin(async move {
                let submission_id = issue_submission_id(
                    &input.relay_origin,
                    &input.channel_id,
                    &input.repo_address,
                    &input.issue_id,
                )?;
                let expected_task_id = task_id(&config, &input.issue_id)?;
                let expected_envelope_digest = config.envelope_digest.clone();
                let expected_resource_plan = config
                    .resource_plan
                    .clone()
                    .ok_or_else(|| "vyzr_workspace_envelope_binding_mismatch".to_string())?;
                let result = client
                    .transport
                    .tool(
                        "development_submit",
                        json!({
                            "schemaVersion": "development-task-submission.v1",
                            "submissionId": submission_id,
                            "envelopeDigest": config.envelope_digest,
                            "spec": {
                                "schemaVersion": "development-task-spec.v1",
                                "title": input.title,
                                "objective": input.objective,
                                "scopes": input.scopes,
                                "checks": config.default_checks,
                                "dependencies": [],
                                "dataClass": config.data_class,
                                "worker": config.default_worker,
                                "reviewer": config.default_reviewer
                            }
                        }),
                    )
                    .await?;
                validate_submission_receipt(
                    result,
                    &expected_task_id,
                    &expected_envelope_digest,
                    &expected_resource_plan,
                )
            })
        },
    )
    .await
}

#[cfg(test)]
#[path = "vyzr_workspace_extra_tests.rs"]
mod extra_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WorkspaceConfig {
        WorkspaceConfig {
            relay_origin: "https://relay.example".into(),
            channel_id: "science-simulations".into(),
            repo_address: "30617:owner:repo".into(),
            project_id: "hudstone".into(),
            principal_id: "buzz-desktop".into(),
            node_executable: "C:\\node.exe".into(),
            node_sha256: "a".repeat(64),
            runtime_root: "C:\\vyzr".into(),
            runtime_revision: "b".repeat(40),
            runtime_script_sha256: "c".repeat(64),
            runtime_tree_sha256: "d".repeat(64),
            repository_path: "C:\\repo".into(),
            state_dir: "C:\\state".into(),
            envelope_path: "C:\\envelope.json".into(),
            envelope_digest: "e".repeat(64),
            codex_executable: "C:\\codex.exe".into(),
            codex_sha256: "f".repeat(64),
            devin_executable: "C:\\devin.exe".into(),
            devin_sha256: "0".repeat(64),
            default_checks: vec!["docs".into()],
            default_worker: "swe-2-direct".into(),
            default_reviewer: "codex-sol".into(),
            data_class: "INTERNAL".into(),
            resource_plan: Some(ResourcePlan {
                max_provider_executions: 4,
                correction_reserve: 1,
                independent_review: "exact_artifact_separate_session".into(),
            }),
        }
    }

    #[test]
    fn derives_stable_task_identity_from_buzz_issue() {
        let config = test_config();
        let issue = "1".repeat(64);
        let submission_digest = sha256_bytes(
            format!(
                "{}\n{}\n{}\n{issue}",
                config.relay_origin, config.channel_id, config.repo_address
            )
            .as_bytes(),
        );
        let expected =
            sha256_bytes(format!("buzz-desktop\nhudstone\nbuzz-{submission_digest}").as_bytes());
        assert_eq!(task_id(&config, &issue).unwrap(), format!("dev-{expected}"));
        assert_eq!(
            task_id(&config, &issue).unwrap(),
            task_id(&config, &issue).unwrap()
        );
    }

    #[test]
    fn rejects_non_event_issue_identity_and_unsafe_lines() {
        assert!(issue_submission_id("relay", "channel", "repo", "../../task").is_err());
        assert!(!safe_line("line\n--worker", 256));
        assert!(!safe_line("line\u{2028}next", 256));
    }

    #[test]
    fn recommendation_must_bind_exact_bytes_to_the_expected_task() {
        let task = format!("dev-{}", "1".repeat(64));
        let bytes = br#"{"schemaVersion":"development-recommendation.v1"}"#;
        let valid = json!({
            "schemaVersion": "development-artifact.v1",
            "taskId": task,
            "kind": "recommendation",
            "byteCount": bytes.len(),
            "digest": sha256_bytes(bytes),
            "contentBase64": BASE64.encode(bytes)
        });
        assert!(validate_recommendation_artifact(valid.clone(), &task).is_ok());
        let mut wrong_task = valid.clone();
        wrong_task["taskId"] = Value::String(format!("dev-{}", "2".repeat(64)));
        assert!(validate_recommendation_artifact(wrong_task, &task).is_err());
        let mut wrong_content = valid;
        wrong_content["contentBase64"] = Value::String(BASE64.encode(b"different"));
        assert!(validate_recommendation_artifact(wrong_content, &task).is_err());
    }

    #[test]
    fn command_executes_the_pinned_runtime_without_a_shell() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().canonicalize().unwrap();
        let mut config = test_config();
        config.runtime_root = runtime.to_string_lossy().into_owned();
        let command = build_validated_mcp_command(&config).unwrap();
        let command = command.as_std();
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            command.get_program(),
            std::ffi::OsStr::new(&config.node_executable)
        );
        assert_eq!(command.get_current_dir(), Some(runtime.as_path()));
        assert_eq!(
            args,
            vec![
                runtime
                    .join("scripts/orchestrator/development-mcp.mjs")
                    .to_string_lossy()
                    .into_owned(),
                "--repo".into(),
                config.repository_path,
                "--state-dir".into(),
                config.state_dir,
                "--envelope".into(),
                config.envelope_path,
                "--envelope-digest".into(),
                config.envelope_digest,
                "--principal".into(),
                config.principal_id,
                "--execution-driver".into(),
                "attended_local".into(),
                "--codex-exe".into(),
                config.codex_executable,
                "--devin-exe".into(),
                config.devin_executable,
            ]
        );
    }

    fn valid_receipt(config: &WorkspaceConfig, task: &str) -> Value {
        json!({
            "schemaVersion": "development-task-submission-receipt.v1",
            "outcome": "accepted_or_replayed",
            "taskId": task,
            "state": "implementing",
            "envelopeDigest": config.envelope_digest,
            "resourcePlan": {
                "maxProviderExecutions": 4,
                "correctionReserve": 1,
                "independentReview": "exact_artifact_separate_session"
            },
            "executionDriver": "scheduled"
        })
    }

    #[test]
    fn submission_receipt_requires_exact_authority_bindings() {
        let config = test_config();
        let task = format!("dev-{}", "1".repeat(64));
        assert!(validate_submission_receipt(
            valid_receipt(&config, &task),
            &task,
            &config.envelope_digest,
            config.resource_plan.as_ref().unwrap(),
        )
        .is_ok());
        let mut wrong = valid_receipt(&config, &task);
        wrong["taskId"] = Value::String(format!("dev-{}", "2".repeat(64)));
        assert!(validate_submission_receipt(
            wrong,
            &task,
            &config.envelope_digest,
            config.resource_plan.as_ref().unwrap()
        )
        .is_err());
        let mut unexpected = valid_receipt(&config, &task);
        unexpected["observedModel"] = Value::String("untrusted".into());
        assert!(validate_submission_receipt(
            unexpected,
            &task,
            &config.envelope_digest,
            config.resource_plan.as_ref().unwrap()
        )
        .is_err());

        let mut wrong_plan = valid_receipt(&config, &task);
        wrong_plan["resourcePlan"]["maxProviderExecutions"] = json!(5);
        assert!(validate_submission_receipt(
            wrong_plan,
            &task,
            &config.envelope_digest,
            config.resource_plan.as_ref().unwrap()
        )
        .is_err());
    }

    #[test]
    fn runtime_tree_digest_is_stable_and_detects_dependency_drift() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("scripts/orchestrator")).unwrap();
        std::fs::write(
            directory
                .path()
                .join("scripts/orchestrator/development-mcp.mjs"),
            b"import './store.mjs';\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("scripts/orchestrator/store.mjs"),
            b"export const value = 1;\n",
        )
        .unwrap();
        let first = runtime_tree_digest(directory.path()).unwrap();
        assert_eq!(first, runtime_tree_digest(directory.path()).unwrap());
        std::fs::write(
            directory.path().join("scripts/orchestrator/store.mjs"),
            b"export const value = 2;\n",
        )
        .unwrap();
        assert_ne!(first, runtime_tree_digest(directory.path()).unwrap());
    }

    fn test_transport() -> (McpTransport, tokio::io::DuplexStream) {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (reader, writer) = tokio::io::split(client);
        (
            McpTransport {
                stdin: Box::new(writer),
                stdout: BufReader::new(Box::new(reader)),
                next_id: 1,
                config_digest: "config".into(),
            },
            server,
        )
    }

    #[tokio::test]
    async fn tool_rejects_mcp_error_even_with_structured_content() {
        let (mut transport, server) = test_transport();
        let (reader, mut writer) = tokio::io::split(server);
        let mut reader = BufReader::new(reader);
        let server_task = tokio::spawn(async move {
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            let response = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [{ "type": "text", "text": "{\"outcome\":\"accepted_or_replayed\"}" }],
                    "structuredContent": { "outcome": "accepted_or_replayed" },
                    "isError": true
                }
            });
            writer
                .write_all(&serde_json::to_vec(&response).unwrap())
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        });
        assert_eq!(
            transport
                .tool("development_submit", json!({}))
                .await
                .unwrap_err(),
            "vyzr_workspace_tool_failed"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn tool_rejects_content_that_does_not_match_structured_content() {
        let (mut transport, server) = test_transport();
        let (reader, mut writer) = tokio::io::split(server);
        let mut reader = BufReader::new(reader);
        let server_task = tokio::spawn(async move {
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            let response = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [{ "type": "text", "text": "{\"outcome\":\"different\"}" }],
                    "structuredContent": { "outcome": "accepted_or_replayed" },
                    "isError": false
                }
            });
            writer
                .write_all(&serde_json::to_vec(&response).unwrap())
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        });
        assert_eq!(
            transport
                .tool("development_submit", json!({}))
                .await
                .unwrap_err(),
            "vyzr_workspace_tool_result_invalid"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn transport_rejects_an_oversized_response_before_parsing() {
        let (mut transport, server) = test_transport();
        let (reader, mut writer) = tokio::io::split(server);
        let mut reader = BufReader::new(reader);
        let server_task = tokio::spawn(async move {
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            writer
                .write_all(&vec![b'x'; MAX_RPC_BYTES + 1])
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        });
        assert_eq!(
            transport
                .request("development_tasks", json!({}))
                .await
                .unwrap_err(),
            "vyzr_workspace_response_too_large"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn transport_rejects_stale_response_identity() {
        let (mut transport, server) = test_transport();
        let (reader, mut writer) = tokio::io::split(server);
        let mut reader = BufReader::new(reader);
        let server_task = tokio::spawn(async move {
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            writer
                .write_all(br#"{"jsonrpc":"2.0","id":99,"result":{}}"#)
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        });
        assert_eq!(
            transport
                .request("development_tasks", json!({}))
                .await
                .unwrap_err(),
            "vyzr_workspace_response_mismatch"
        );
        server_task.await.unwrap();
    }
}
