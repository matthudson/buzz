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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

const CONFIG_ENV: &str = "BUZZ_VYZR_WORKSPACE_CONFIG";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_RPC_BYTES: usize = 512 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(15);

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
    repo_address: String,
    project_id: String,
    principal_id: String,
    node_executable: String,
    node_sha256: String,
    runtime_root: String,
    runtime_revision: String,
    runtime_script_sha256: String,
    repository_path: String,
    state_dir: String,
    envelope_path: String,
    envelope_digest: String,
    codex_executable: String,
    devin_executable: String,
    default_checks: Vec<String>,
    default_worker: String,
    default_reviewer: String,
    data_class: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VyzrWorkspaceProjection {
    schema_version: &'static str,
    repo_address: String,
    task_id: String,
    requested_worker: String,
    requested_reviewer: String,
    requested_checks: Vec<String>,
    data_class: String,
    task: Option<Value>,
    events: Vec<Value>,
    recommendation: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitVyzrProjectTaskInput {
    repo_address: String,
    issue_id: String,
    title: String,
    objective: String,
    scopes: Vec<String>,
}

struct VyzrMcpClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
    if !safe_line(&config.repo_address, 1024)
        || !safe_identifier(&config.project_id, 128)
        || !safe_identifier(&config.principal_id, 101)
        || !is_hex(&config.node_sha256, 64)
        || !is_hex(&config.runtime_revision, 40)
        || !is_hex(&config.runtime_script_sha256, 64)
        || !is_hex(&config.envelope_digest, 64)
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
    let repository = absolute_existing(&config.repository_path, true)?;
    let state = absolute_existing(&config.state_dir, true)?;
    let envelope = absolute_existing(&config.envelope_path, false)?;
    let codex = absolute_existing(&config.codex_executable, false)?;
    let devin = absolute_existing(&config.devin_executable, false)?;
    let envelope_value: Value = serde_json::from_slice(
        &std::fs::read(&envelope).map_err(|_| "vyzr_workspace_envelope_unavailable".to_string())?,
    )
    .map_err(|_| "vyzr_workspace_envelope_invalid".to_string())?;
    if envelope_value.get("schemaVersion").and_then(Value::as_str)
        != Some("development-project-envelope.v1")
        || envelope_value.get("projectId").and_then(Value::as_str)
            != Some(config.project_id.as_str())
        || envelope_value.get("principalId").and_then(Value::as_str)
            != Some(config.principal_id.as_str())
        || envelope_value.get("revision").and_then(Value::as_str)
            != Some(config.runtime_revision.as_str())
    {
        return Err("vyzr_workspace_envelope_binding_mismatch".to_string());
    }
    config.node_executable = node.to_string_lossy().into_owned();
    config.runtime_root = runtime.to_string_lossy().into_owned();
    config.repository_path = repository.to_string_lossy().into_owned();
    config.state_dir = state.to_string_lossy().into_owned();
    config.envelope_path = envelope.to_string_lossy().into_owned();
    config.codex_executable = codex.to_string_lossy().into_owned();
    config.devin_executable = devin.to_string_lossy().into_owned();
    Ok(config)
}

fn load_workspace(repo_address: &str) -> Result<(WorkspaceConfig, String), String> {
    if !safe_line(repo_address, 1024) {
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
        .filter(|workspace| workspace.repo_address == repo_address)
        .collect();
    if matches.len() != 1 {
        return Err("vyzr_workspace_mapping_unavailable".to_string());
    }
    let workspace = validate_workspace(matches.into_iter().next().expect("one mapping"))?;
    Ok((workspace, sha256_bytes(&bytes)))
}

fn issue_submission_id(issue_id: &str) -> Result<String, String> {
    if !is_hex(issue_id, 64) {
        return Err("vyzr_workspace_issue_id_invalid".to_string());
    }
    Ok(format!("buzz-{issue_id}"))
}

fn task_id(config: &WorkspaceConfig, issue_id: &str) -> Result<String, String> {
    let submission = issue_submission_id(issue_id)?;
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

async fn read_bounded_line(reader: &mut BufReader<ChildStdout>) -> Result<Vec<u8>, String> {
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
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            config_digest,
        };
        let initialized = client
            .request("initialize", json!({
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
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

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
        self.write_value(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "vyzr_workspace_request_id_exhausted".to_string())?;
        self.write_value(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await?;
        let bytes = timeout(RPC_TIMEOUT, read_bounded_line(&mut self.stdout))
            .await
            .map_err(|_| "vyzr_workspace_transport_timeout".to_string())??;
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
        response
            .pointer("/result/structuredContent")
            .cloned()
            .ok_or_else(|| "vyzr_workspace_tool_result_invalid".to_string())
    }
}

fn build_mcp_command(config: &WorkspaceConfig) -> Result<Command, String> {
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
    let (config, config_digest) = load_workspace(repo_address)?;
    let mut clients = state.clients.lock().await;
    if let Some(existing) = clients.get(repo_address) {
        if existing.config_digest != config_digest {
            return Err("vyzr_workspace_configuration_changed_restart_required".to_string());
        }
    } else {
        let client = VyzrMcpClient::launch(&config, config_digest).await?;
        clients.insert(repo_address.to_string(), client);
    }
    let client = clients.get_mut(repo_address).expect("client inserted");
    operation(client, config).await
}

#[tauri::command]
pub async fn get_vyzr_project_task(
    repo_address: String,
    issue_id: String,
    state: State<'_, VyzrWorkspaceState>,
) -> Result<VyzrWorkspaceProjection, String> {
    with_client(&state, &repo_address, |client, config| {
        Box::pin(async move {
            let expected_task_id = task_id(&config, &issue_id)?;
            let view = client.tool("development_tasks", json!({})).await?;
            let tasks = view
                .get("tasks")
                .and_then(Value::as_array)
                .ok_or_else(|| "vyzr_workspace_projection_invalid".to_string())?;
            let task = tasks
                .iter()
                .find(|item| {
                    item.get("taskId").and_then(Value::as_str) == Some(expected_task_id.as_str())
                })
                .cloned();
            let mut events = Vec::new();
            let mut recommendation = None;
            if let Some(ref current) = task {
                let last_sequence = current
                    .pointer("/lastVerifiedUpdate/sequence")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "vyzr_workspace_projection_invalid".to_string())?;
                let page = client
                    .tool(
                        "development_events",
                        json!({
                            "taskId": expected_task_id,
                            "afterSequence": last_sequence.saturating_sub(8),
                            "limit": 8
                        }),
                    )
                    .await?;
                events = page
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| "vyzr_workspace_events_invalid".to_string())?;
                if current.get("state").and_then(Value::as_str) == Some("recommended") {
                    let artifact = client
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
                task_id: expected_task_id,
                requested_worker: config.default_worker,
                requested_reviewer: config.default_reviewer,
                requested_checks: config.default_checks,
                data_class: config.data_class,
                task,
                events,
                recommendation,
            })
        })
    })
    .await
}

#[tauri::command]
pub async fn submit_vyzr_project_task(
    input: SubmitVyzrProjectTaskInput,
    state: State<'_, VyzrWorkspaceState>,
) -> Result<Value, String> {
    if !safe_line(&input.title, 256)
        || input.objective.len() > 16_000
        || input.objective.chars().any(|ch| ch == '\0')
        || input.scopes.is_empty()
        || input.scopes.len() > 64
        || input.scopes.iter().any(|scope| !safe_line(scope, 256))
    {
        return Err("vyzr_workspace_submission_invalid".to_string());
    }
    let repo_address = input.repo_address.clone();
    with_client(&state, &repo_address, |client, config| {
        Box::pin(async move {
            let submission_id = issue_submission_id(&input.issue_id)?;
            client
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
                .await
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WorkspaceConfig {
        WorkspaceConfig {
            repo_address: "30617:owner:repo".into(),
            project_id: "hudstone".into(),
            principal_id: "buzz-desktop".into(),
            node_executable: "C:\\node.exe".into(),
            node_sha256: "a".repeat(64),
            runtime_root: "C:\\vyzr".into(),
            runtime_revision: "b".repeat(40),
            runtime_script_sha256: "c".repeat(64),
            repository_path: "C:\\repo".into(),
            state_dir: "C:\\state".into(),
            envelope_path: "C:\\envelope.json".into(),
            envelope_digest: "d".repeat(64),
            codex_executable: "C:\\codex.exe".into(),
            devin_executable: "C:\\devin.exe".into(),
            default_checks: vec!["docs".into()],
            default_worker: "swe-2-direct".into(),
            default_reviewer: "codex-sol".into(),
            data_class: "INTERNAL".into(),
        }
    }

    #[test]
    fn derives_stable_task_identity_from_buzz_issue() {
        let config = test_config();
        let issue = "1".repeat(64);
        let expected = sha256_bytes(format!("buzz-desktop\nhudstone\nbuzz-{issue}").as_bytes());
        assert_eq!(task_id(&config, &issue).unwrap(), format!("dev-{expected}"));
        assert_eq!(
            task_id(&config, &issue).unwrap(),
            task_id(&config, &issue).unwrap()
        );
    }

    #[test]
    fn rejects_non_event_issue_identity_and_unsafe_lines() {
        assert!(issue_submission_id("../../task").is_err());
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
        let command = build_mcp_command(&config).unwrap();
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
}
