use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

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
    let Ok(agent_bin) = resolve_agent_binary() else {
        eprintln!(
            "error: awidat-agent binary not found. Build it with `cargo build -p awidat-agent-cli --bin awidat-agent`, or set AWIDAT_AGENT_BIN."
        );
        return ExitCode::from(1);
    };
    let status = std::process::Command::new(agent_bin).args(args).status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("error: failed to run awidat-agent: {err}");
            ExitCode::from(1)
        }
    }
}

fn resolve_agent_binary() -> Result<PathBuf, std::env::VarError> {
    if let Ok(path) = std::env::var("AWIDAT_AGENT_BIN") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|_| std::env::VarError::NotPresent)?;
    let parent = exe.parent().ok_or(std::env::VarError::NotPresent)?;
    let name = if cfg!(windows) {
        "awidat-agent.exe"
    } else {
        "awidat-agent"
    };
    let candidate = parent.join(name);
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(std::env::VarError::NotPresent)
    }
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
}
