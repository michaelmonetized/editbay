use editbay_core::{DocumentVersion, Project, load, recover_copy, save, save_new};
use rmcp::{
    ClientServiceExt, RoleClient,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ProtocolVersion},
    service::{ClientLifecycleMode, RunningService},
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::path::Path;

async fn start(
    path: &Path,
    recovery: &Path,
    modern: bool,
) -> RunningService<RoleClient, ClientConfig> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_editbay-mcp"));
    command.arg(path).arg(recovery);
    let transport = TokioChildProcess::new(command).unwrap();
    let mode = if modern {
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        }
    } else {
        ClientLifecycleMode::Initialize
    };
    let mut config = ClientConfig::default();
    config.protocol_version = ProtocolVersion::V_2025_11_25;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        config.serve_with_lifecycle(transport, mode),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn call(
    client: &RunningService<RoleClient, ClientConfig>,
    name: &str,
    args: Value,
) -> CallToolResult {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

fn success(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    result.structured_content.unwrap()
}

#[tokio::test]
async fn modern_and_legacy_clients_edit_undo_checkpoint_and_restart_real_files() {
    for modern in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("A client job.editbay");
        let recovery = directory.path().join("Recovery folder");
        let project = Project::new("Original").unwrap();
        save_new(&project, &path).unwrap();
        let client = start(&path, &recovery, modern).await;
        let tools = client.list_all_tools().await.unwrap();
        let mut names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "apply_commands",
                "create_checkpoint",
                "inspect_document",
                "inspect_recoveries",
                "redo",
                "undo"
            ]
        );
        for tool in tools {
            let schema = serde_json::to_string(&tool.input_schema).unwrap();
            assert!(!schema.contains("destination"));
            assert!(!schema.contains("recovery_root"));
        }
        let inspected = success(call(&client, "inspect_document", json!({})).await);
        assert_eq!(inspected["disk"]["status"], "matches");
        assert_eq!(inspected["capabilities"]["export"], false);
        let version = inspected["version"].clone();
        let invalid = call(
            &client,
            "apply_commands",
            json!({
                "expected":version, "label":"Invalid group", "commands":[
                    {"kind":"rename_project","name":"Partial edit"},
                    {"kind":"rename_project","name":" "}
                ]
            }),
        )
        .await;
        assert_eq!(invalid.is_error, Some(true));
        assert_eq!(load(&path).unwrap(), project);
        let changed = success(
            call(
                &client,
                "apply_commands",
                json!({
                    "expected":version, "label":"Name", "commands":[
                        {"kind":"rename_project","name":"Intermediate"},
                        {"kind":"rename_project","name":"Approved revision"}
                    ]
                }),
            )
            .await,
        );
        assert_eq!(changed["saved"], true);
        assert_eq!(load(&path).unwrap().name, "Approved revision");
        assert_eq!(load(&path).unwrap().revision, 1);
        assert_eq!(
            call(&client, "undo", json!({"expected":version}))
                .await
                .is_error,
            Some(true)
        );
        let undo = success(
            call(
                &client,
                "undo",
                json!({"expected":changed["receipt"]["after"]}),
            )
            .await,
        );
        assert_eq!(load(&path).unwrap().name, "Original");
        assert_eq!(load(&path).unwrap().revision, 2);
        let redo = success(
            call(
                &client,
                "redo",
                json!({"expected":undo["receipt"]["after"]}),
            )
            .await,
        );
        assert_eq!(load(&path).unwrap().name, "Approved revision");
        assert_eq!(load(&path).unwrap().revision, 3);
        let checkpoint = success(
            call(
                &client,
                "create_checkpoint",
                json!({"expected":redo["receipt"]["after"]}),
            )
            .await,
        );
        let catalog = success(call(&client, "inspect_recoveries", json!({})).await);
        assert_eq!(catalog["valid"].as_array().unwrap().len(), 1);
        let recovered_path = directory.path().join("Recovered copy.editbay");
        let recovered =
            recover_copy(checkpoint["checkpoint"].as_str().unwrap(), &recovered_path).unwrap();
        assert_eq!(recovered.name, "Approved revision");
        assert_ne!(recovered.id, project.id);
        assert_eq!(load(&path).unwrap().revision, 3);
        client.cancel().await.unwrap();
        let restarted = start(&path, &recovery, modern).await;
        let inspected = success(call(&restarted, "inspect_document", json!({})).await);
        assert_eq!(inspected["document"]["name"], "Approved revision");
        assert_eq!(inspected["history"]["undo"], Value::Null);
        assert_eq!(inspected["disk"]["status"], "matches");
        restarted.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn outside_edits_and_deletion_preserve_history_and_never_resurrect_the_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Job.editbay");
    let recovery = directory.path().join("Recovery");
    let mut external = Project::new("Start").unwrap();
    save_new(&external, &path).unwrap();
    let client = start(&path, &recovery, true).await;
    let changed = success(
        call(
            &client,
            "apply_commands",
            json!({
                "expected":DocumentVersion::of(&external), "label":"Local name",
                "commands":[{"kind":"rename_project","name":"Local edit"}]
            }),
        )
        .await,
    );
    external.rename("Outside edit").unwrap();
    save(&external, &path).unwrap();
    let rejected = call(
        &client,
        "undo",
        json!({"expected":changed["receipt"]["after"]}),
    )
    .await;
    assert_eq!(rejected.is_error, Some(true));
    assert_eq!(load(&path).unwrap(), external);
    let inspected = success(call(&client, "inspect_document", json!({})).await);
    assert_eq!(inspected["document"]["name"], "Local edit");
    assert_eq!(inspected["version"]["revision"], 1);
    assert_eq!(inspected["history"]["undo"], "Local name");
    assert_eq!(inspected["disk"]["status"], "changed");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        call(&client, "undo", json!({"expected":inspected["version"]}))
            .await
            .is_error,
        Some(true)
    );
    assert!(!path.exists());
    let checkpoint = success(
        call(
            &client,
            "create_checkpoint",
            json!({"expected":inspected["version"]}),
        )
        .await,
    );
    let rescued = directory.path().join("Rescued.editbay");
    assert_eq!(
        recover_copy(checkpoint["checkpoint"].as_str().unwrap(), &rescued)
            .unwrap()
            .name,
        "Local edit"
    );
    assert!(!path.exists());
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn two_actual_server_processes_cannot_acknowledge_conflicting_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Shared.editbay");
    let recovery = directory.path().join("Recovery");
    let project = Project::new("Before").unwrap();
    save_new(&project, &path).unwrap();
    let first = start(&path, &recovery, true).await;
    let second = start(&path, &recovery, false).await;
    let expected = DocumentVersion::of(&project);
    let (a, b) = tokio::join!(
        call(
            &first,
            "apply_commands",
            json!({"expected":expected,"label":"First","commands":[{"kind":"rename_project","name":"First"}]})
        ),
        call(
            &second,
            "apply_commands",
            json!({"expected":expected,"label":"Second","commands":[{"kind":"rename_project","name":"Second"}]})
        )
    );
    assert_ne!(a.is_error, b.is_error);
    let saved = load(&path).unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        saved.name,
        if a.is_error == Some(false) {
            "First"
        } else {
            "Second"
        }
    );
    first.cancel().await.unwrap();
    second.cancel().await.unwrap();
}
