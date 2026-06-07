use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

struct AgentCommand {
    program: OsString,
    args: Vec<OsString>,
}

pub struct AgentChatArgs {
    pub project_root: PathBuf,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub yolo: bool,
    pub config_overrides: Vec<String>,
}

pub struct AgentResumeArgs {
    pub selector: Option<String>,
    pub model: Option<String>,
}

pub struct AgentPreparedRunArgs {
    pub project_root: PathBuf,
    pub prompt: String,
    pub display_name: &'static str,
    pub model: Option<String>,
}

pub fn run_chat(args: AgentChatArgs) -> ExitCode {
    run_agent(agent_chat_args(args))
}

pub fn run_tui(args: AgentChatArgs) -> ExitCode {
    let mut agent_args = agent_chat_args(args);
    if let Some(first) = agent_args.first_mut() {
        *first = OsString::from("tui");
    }
    run_agent(agent_args)
}

pub fn run_resume(args: AgentResumeArgs) -> ExitCode {
    run_agent(agent_resume_args(args))
}

pub fn run_prepared(args: AgentPreparedRunArgs) -> ExitCode {
    run_agent(agent_prepared_run_args(args))
}

fn run_agent(args: Vec<OsString>) -> ExitCode {
    let Ok(command) = resolve_agent_command(args) else {
        eprintln!(
            "error: awidat-agent binary not found. Build it with `cargo build -p awidat-agent-cli --bin awidat-agent`, run from the Awidat workspace, or set AWIDAT_AGENT_BIN."
        );
        return ExitCode::from(1);
    };
    let status = std::process::Command::new(command.program)
        .args(command.args)
        .status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("error: failed to run awidat-agent: {err}");
            ExitCode::from(1)
        }
    }
}

fn resolve_agent_command(args: Vec<OsString>) -> Result<AgentCommand, std::env::VarError> {
    if let Ok(path) = std::env::var("AWIDAT_AGENT_BIN") {
        return Ok(AgentCommand {
            program: PathBuf::from(path).into_os_string(),
            args,
        });
    }
    let exe = std::env::current_exe().map_err(|_| std::env::VarError::NotPresent)?;
    let parent = exe.parent().ok_or(std::env::VarError::NotPresent)?;
    let name = if cfg!(windows) {
        "awidat-agent.exe"
    } else {
        "awidat-agent"
    };
    resolve_agent_command_with(Some(parent.join(name)), args)
}

fn resolve_agent_command_with(
    sibling_candidate: Option<PathBuf>,
    args: Vec<OsString>,
) -> Result<AgentCommand, std::env::VarError> {
    if let Some(candidate) = sibling_candidate
        && candidate.exists()
    {
        return Ok(AgentCommand {
            program: candidate.into_os_string(),
            args,
        });
    }

    let manifest_path = workspace_manifest_path();
    let agent_manifest_path = workspace_root()
        .join("crates")
        .join("agent-cli")
        .join("Cargo.toml");
    if manifest_path.exists() && agent_manifest_path.exists() {
        let mut cargo_args = vec![
            OsString::from("run"),
            OsString::from("--manifest-path"),
            OsString::from(manifest_path.display().to_string()),
            OsString::from("-p"),
            OsString::from("awidat-agent-cli"),
            OsString::from("--bin"),
            OsString::from("awidat-agent"),
            OsString::from("--"),
        ];
        cargo_args.extend(args);
        return Ok(AgentCommand {
            program: OsString::from("cargo"),
            args: cargo_args,
        });
    }

    Err(std::env::VarError::NotPresent)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn workspace_manifest_path() -> PathBuf {
    workspace_root().join("Cargo.toml")
}

fn agent_chat_args(args: AgentChatArgs) -> Vec<OsString> {
    let mut out = vec![OsString::from("chat"), args.project_root.into_os_string()];
    if let Some(prompt) = args.prompt {
        out.push(prompt.into());
    }
    push_optional(&mut out, "--model", args.model);
    if args.yolo {
        out.push("--yolo".into());
    }
    for config in args.config_overrides {
        out.push("--config".into());
        out.push(config.into());
    }
    out
}

fn agent_resume_args(args: AgentResumeArgs) -> Vec<OsString> {
    let mut out = vec![OsString::from("resume")];
    if let Some(selector) = args.selector {
        out.push(selector.into());
    }
    push_optional(&mut out, "--model", args.model);
    out
}

fn agent_prepared_run_args(args: AgentPreparedRunArgs) -> Vec<OsString> {
    let mut out = vec![
        OsString::from("run-prepared"),
        OsString::from("--project"),
        args.project_root.into_os_string(),
        OsString::from("--prompt"),
        OsString::from(args.prompt),
        OsString::from("--display-name"),
        OsString::from(args.display_name),
    ];
    push_optional(&mut out, "--model", args.model);
    out
}

fn push_optional(out: &mut Vec<OsString>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        out.push(flag.into());
        out.push(value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_args_preserve_existing_cli_shape() {
        let args = agent_chat_args(AgentChatArgs {
            project_root: "project".into(),
            prompt: Some("cut the intro".into()),
            model: Some("gpt-test".into()),
            yolo: true,
            config_overrides: vec!["approval_policy=\"never\"".into()],
        });

        assert_eq!(
            args,
            vec![
                OsString::from("chat"),
                OsString::from("project"),
                OsString::from("cut the intro"),
                OsString::from("--model"),
                OsString::from("gpt-test"),
                OsString::from("--yolo"),
                OsString::from("--config"),
                OsString::from("approval_policy=\"never\""),
            ]
        );
    }

    #[test]
    fn prepared_run_args_preserve_prompt_project_and_display_name() {
        let args = agent_prepared_run_args(AgentPreparedRunArgs {
            project_root: "project".into(),
            prompt: "use the sound-design skill".into(),
            display_name: "awidat skills run",
            model: Some("gpt-test".into()),
        });

        assert_eq!(
            args,
            vec![
                OsString::from("run-prepared"),
                OsString::from("--project"),
                OsString::from("project"),
                OsString::from("--prompt"),
                OsString::from("use the sound-design skill"),
                OsString::from("--display-name"),
                OsString::from("awidat skills run"),
                OsString::from("--model"),
                OsString::from("gpt-test"),
            ]
        );
    }

    #[test]
    fn missing_sibling_agent_falls_back_to_workspace_cargo_run() {
        let args = vec![OsString::from("tui"), OsString::from("project")];
        let command =
            resolve_agent_command_with(Some(PathBuf::from("/missing/awidat-agent")), args)
                .expect("workspace cargo fallback should be available");

        assert_eq!(command.program, OsString::from("cargo"));
        assert_eq!(
            command.args,
            vec![
                OsString::from("run"),
                OsString::from("--manifest-path"),
                OsString::from(workspace_manifest_path().display().to_string()),
                OsString::from("-p"),
                OsString::from("awidat-agent-cli"),
                OsString::from("--bin"),
                OsString::from("awidat-agent"),
                OsString::from("--"),
                OsString::from("tui"),
                OsString::from("project"),
            ]
        );
    }
}
