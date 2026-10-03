use editbay_automation::Automation;
use rmcp::ServiceExt;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("editbay-mcp: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let project = args.next().map(PathBuf::from);
    let recovery = args.next().map(PathBuf::from);
    let (Some(project), Some(recovery), None) = (project, recovery, args.next()) else {
        return Err("usage: editbay-mcp PROJECT.editbay RECOVERY_DIRECTORY".into());
    };
    Automation::open(&project, &recovery)?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
