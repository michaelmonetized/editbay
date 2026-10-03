//! Scoped automation uses the same validated commands and durable saves as the application.

use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, checkpoint, load, recovery_catalog,
    save_if_unchanged,
};
use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug)]
struct Session {
    editor: DocumentEditor,
    project_path: PathBuf,
    recovery_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Automation {
    session: Arc<Mutex<Session>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Apply {
    expected: DocumentVersion,
    label: String,
    commands: Vec<DocumentCommand>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AtVersion {
    expected: DocumentVersion,
}

impl Automation {
    /// Open one project and grant access to one recovery directory.
    /// `project_path` must name an existing regular project; `recovery_root` is
    /// created if absent. Returns a scoped session, without modifying the project.
    pub fn open(project_path: &Path, recovery_root: &Path) -> editbay_core::Result<Self> {
        if !fs::symlink_metadata(project_path)?.is_file() {
            return Err(editbay_core::Error::Invalid(
                "automation project must be a regular file, not a symlink".into(),
            ));
        }
        let project_path = fs::canonicalize(project_path)?;
        let editor = DocumentEditor::new(load(&project_path)?)?;
        fs::create_dir_all(recovery_root)?;
        if !fs::symlink_metadata(recovery_root)?.is_dir() {
            return Err(editbay_core::Error::Invalid(
                "recovery scope must be a directory, not a symlink".into(),
            ));
        }
        let recovery_root = fs::canonicalize(recovery_root)?;
        Ok(Self {
            session: Arc::new(Mutex::new(Session {
                editor,
                project_path,
                recovery_root,
            })),
        })
    }

    async fn run(
        &self,
        operation: impl FnOnce(&mut Session) -> editbay_core::Result<Value> + Send + 'static,
    ) -> CallToolResult {
        let session = self.session.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut session = session.lock().map_err(|_| {
                editbay_core::Error::Invalid("document session failed; restart automation".into())
            })?;
            operation(&mut session)
        })
        .await;
        match result {
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(error)) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!(
                "document worker failed: {error}"
            ))]),
        }
    }
}

fn mutate(
    session: &mut Session,
    operation: impl FnOnce(&mut DocumentEditor) -> editbay_core::Result<editbay_core::CommandReceipt>,
) -> editbay_core::Result<Value> {
    let mut candidate = session.editor.clone();
    let receipt = operation(&mut candidate)?;
    save_if_unchanged(
        candidate.project(),
        &session.project_path,
        session.editor.project(),
    )?;
    session.editor = candidate;
    Ok(json!({"receipt": receipt, "saved": true}))
}

#[tool_router]
impl Automation {
    #[tool(
        description = "Inspect the opened document, current version, undo history, scopes and implemented capabilities",
        annotations(read_only_hint = true)
    )]
    async fn inspect_document(&self) -> CallToolResult {
        self.run(|session| {
            let (undo, redo) = session.editor.history();
            let disk = match load(&session.project_path) {
                Ok(project) if project == *session.editor.project() => json!({"status":"matches"}),
                Ok(project) => json!({"status":"changed", "version":DocumentVersion::of(&project)}),
                Err(error) => json!({"status":"unavailable", "error":error.to_string()}),
            };
            Ok(json!({
                "document": session.editor.project(),
                "version": DocumentVersion::of(session.editor.project()),
                "history": {"undo": undo, "redo": redo, "limit":128, "persisted":false},
                "scope": {"project":session.project_path, "recovery":session.recovery_root},
                "disk": disk,
                "capabilities": {
                    "commands":["rename_project"], "atomic_groups":true,
                    "undo":true, "redo":true, "checkpoint":true,
                    "preview":false, "export":false, "media_import":false,
                    "jobs":false, "cloud":false
                }
            }))
        })
        .await
    }

    #[tool(
        description = "Apply 1..64 typed commands as one undo step and durably save; expected project identity and revision are required",
        annotations(destructive_hint = false)
    )]
    async fn apply_commands(&self, Parameters(args): Parameters<Apply>) -> CallToolResult {
        self.run(move |session| {
            mutate(session, |editor| {
                editor.apply(args.expected, args.label, &args.commands)
            })
        })
        .await
    }

    #[tool(
        description = "Undo the last command group and durably save with a new revision",
        annotations(destructive_hint = false)
    )]
    async fn undo(&self, Parameters(args): Parameters<AtVersion>) -> CallToolResult {
        self.run(move |session| mutate(session, |editor| editor.undo(args.expected)))
            .await
    }

    #[tool(
        description = "Redo the last undone command group and durably save with a new revision",
        annotations(destructive_hint = false)
    )]
    async fn redo(&self, Parameters(args): Parameters<AtVersion>) -> CallToolResult {
        self.run(move |session| mutate(session, |editor| editor.redo(args.expected)))
            .await
    }

    #[tool(
        description = "Write an immutable checksummed recovery snapshot of the current version into the granted recovery folder",
        annotations(destructive_hint = false)
    )]
    async fn create_checkpoint(&self, Parameters(args): Parameters<AtVersion>) -> CallToolResult {
        self.run(move |session| {
            let current = DocumentVersion::of(session.editor.project());
            if args.expected != current {
                return Err(editbay_core::Error::Invalid(
                    "checkpoint version does not own the current document".into(),
                ));
            }
            let path = checkpoint(
                session.editor.project(),
                Some(&session.project_path),
                &session.recovery_root,
            )?;
            Ok(json!({"version":current, "checkpoint":path}))
        })
        .await
    }

    #[tool(
        description = "Inspect valid and corrupt recovery snapshots in the granted recovery folder",
        annotations(read_only_hint = true)
    )]
    async fn inspect_recoveries(&self) -> CallToolResult {
        self.run(|session| {
            Ok(serde_json::to_value(recovery_catalog(
                &session.recovery_root,
            )?)?)
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for Automation {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("editbay", env!("CARGO_PKG_VERSION")))
            .with_instructions("Access is limited to the project and recovery folder selected at startup. Read inspect_document before issuing commands. Every mutation requires its expected project identity and revision and succeeds only after a durable save. Only listed tools and commands are implemented.")
    }
}
