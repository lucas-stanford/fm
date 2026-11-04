//! `nodes:`: a step handed to its own agent CLI and model, and `fm node <id>`, which runs it.
//!
//! The body runs the chart as always. When it reaches a step listed in `nodes`, it runs
//! `fm node <id> -m "<what the step needs>"`: this runs the step's section as its own turn through
//! the node's agent, with the input block the body got, in the body's folder (or `--cwd`), within
//! what is left of the run's timeout. The answer comes back on stdout. Only the body moves the run
//! along the chart; the node reports on its own step.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use fm_model::{
    AgentCli, CliId, CliModel, CliSettings, Config, Located, NodeAgent, StatePaths, TurnRequest,
};
use fm_spec::step_sections;
use fm_state::{
    NextArgs, SayArgs, StepArgs, progress_next, progress_say, progress_status, progress_step,
};
use serde::{Deserialize, Serialize};

use crate::live::LiveWriter;
use crate::stop::halt_file;
use crate::tracker::{ENV_NODE, ENV_RUN, ENV_STATE_DIR};
use crate::turn::{Transcript, Turn, TurnEnd, run_turn};

/// The agent CLI, model and bound a node runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePlan {
    pub id: String,
    pub cli: CliId,
    pub model: CliModel,
    pub settings: CliSettings,
    /// The node's own `timeout`, when it has one.
    pub timeout: Option<Duration>,
}

/// What `nodes:` says for `id`, or the body's own agent for a step it does not list.
pub fn setting_for(l: &Located, id: &str) -> NodeAgent {
    l.automation
        .nodes
        .iter()
        .find(|n| n.id == id)
        .cloned()
        .unwrap_or(NodeAgent {
            id: id.to_string(),
            agent: None,
            model: None,
            timeout: None,
        })
}

/// Resolve a node: its agent (else the automation's), its model (else the automation's when it is
/// the same agent, else that agent's default). Refuses, never substitutes.
pub fn plan_node(config: &Config, l: &Located, node: &NodeAgent) -> Result<NodePlan, String> {
    let body_cli = config.agent_for(&l.key);
    let cli = match &node.agent {
        Some(name) => name.parse::<CliId>().map_err(|_| {
            format!(
                "nodes.{}.agent: {name} is not an agent CLI fm runs — use {}",
                node.id,
                CliId::ALL.map(|c| c.to_string()).join(", ")
            )
        })?,
        None => body_cli.ok_or_else(|| {
            format!(
                "nodes.{}: no agent CLI — name one with agent:, or fm agent set {} <cli>",
                node.id, l.automation.name
            )
        })?,
    };
    let settings = config.agent_settings(cli);
    let spec = node.model.as_ref().or_else(|| {
        (Some(cli) == body_cli)
            .then_some(l.automation.model.as_ref())
            .flatten()
    });
    let model = fm_agents::adapter(cli)
        .resolve_model(spec, &settings)
        .map_err(|r| format!("nodes.{}: {}", node.id, r.message))?;
    let timeout = node.timeout.as_deref().and_then(fm_spec::parse_duration);
    Ok(NodePlan {
        id: node.id.clone(),
        cli,
        model,
        settings,
        timeout,
    })
}

/// Every node `nodes:` lists, resolved; the first that cannot run here is the reason.
pub fn plan_nodes(config: &Config, l: &Located) -> Result<Vec<NodePlan>, String> {
    l.automation
        .nodes
        .iter()
        .map(|n| plan_node(config, l, n))
        .collect()
}

/// What `fm node` needs from the run, saved when the body starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeContext {
    pub key: String,
    /// The folder the body runs in.
    pub cwd: PathBuf,
    /// When the run's timeout ends.
    pub deadline: String,
    /// The input block the body got, as the body got it.
    pub input: String,
}

pub fn context_file(paths: &StatePaths, run_id: &str) -> PathBuf {
    paths.progress_dir().join(format!("{run_id}.node.json"))
}

pub fn save_context(paths: &StatePaths, run_id: &str, ctx: &NodeContext) {
    let file = context_file(paths, run_id);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(ctx) {
        let _ = std::fs::write(file, text);
    }
}

pub fn remove_context(paths: &StatePaths, run_id: &str) {
    let _ = std::fs::remove_file(context_file(paths, run_id));
}

fn load_context(paths: &StatePaths, run_id: &str) -> Option<NodeContext> {
    let text = std::fs::read_to_string(context_file(paths, run_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The prompt a node's agent gets: what it is, the automation's introduction for context, its own
/// section, what the body handed over, the reporting it may do, then the run's input as data.
pub fn node_prompt(
    name: &str,
    intro: &str,
    section: &str,
    id: &str,
    handoff: Option<&str>,
    input: &str,
) -> String {
    let mut parts = vec![
        format!(
            "You are running one step of the automation \"{name}\": the section below with id `{id}`. \
             The run's main agent handed this step to you. Do this step, and only this step, then \
             answer with what you did and found: your answer goes back to the main agent."
        ),
        String::new(),
        "<automation>".to_string(),
        intro.trim().to_string(),
        "</automation>".to_string(),
        String::new(),
        "<step>".to_string(),
        section.trim().to_string(),
        "</step>".to_string(),
    ];
    if let Some(h) = handoff.map(str::trim).filter(|h| !h.is_empty()) {
        parts.extend([
            String::new(),
            "The main agent says:".to_string(),
            "<handoff>".to_string(),
            h.to_string(),
            "</handoff>".to_string(),
        ]);
    }
    parts.extend([
        String::new(),
        format!(
            "<progress>\nReport on this step with `fm say \"text\"` (AUTOMATION_RUN and AUTOMATION_NODE={id} are \
             already set) and record anything you publish with `fm note \"what it is\" --link \"label=https://…\"`. \
             Do not run `fm next` or `fm step`: the main agent decides where the run goes after this step. \
             If `fm` fails, carry on.\n</progress>"
        ),
    ]);
    if !input.trim().is_empty() {
        parts.push(String::new());
        parts.push(input.trim().to_string());
    }
    parts.join("\n")
}

/// What `fm node` is asked.
#[derive(Debug, Clone, Default)]
pub struct NodeRequest {
    pub id: String,
    pub handoff: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// What the node's agent answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAnswer {
    pub answer: String,
    pub agent: String,
    pub model: String,
    pub transcript: PathBuf,
}

/// Why `fm node` did not get an answer. `Refused`: nothing ran. `Failed`: the agent ran and did not
/// answer. `Stopped`: a person stopped the run, or its time ran out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeError {
    Refused(String),
    Failed(String),
    Stopped(String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Refused(m) | NodeError::Failed(m) | NodeError::Stopped(m) => f.write_str(m),
        }
    }
}

/// Everything `run_node` needs from the host.
pub struct NodeDeps<'a> {
    pub paths: &'a StatePaths,
    pub config: &'a Config,
    /// The run's automation, found by the key its progress names.
    pub located: &'a Located,
    /// `PATH` with fm's folder first, so the node's agent can run `fm say`.
    pub path_env: Vec<(String, String)>,
    pub poll: Duration,
}

fn say(paths: &StatePaths, run_id: &str, node: &str, text: &str, now: DateTime<Utc>) {
    let _ = progress_say(
        paths,
        Some(run_id),
        &SayArgs {
            text: text.to_string(),
            node: Some(node.to_string()),
            env_node: None,
            request_id: None,
        },
        now,
    );
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!(
            "{}…",
            flat.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

/// A fresh transcript file for this node's turn: `sessions/<run>-<node>-<n>.jsonl`.
fn transcript_file(paths: &StatePaths, run_id: &str, id: &str) -> PathBuf {
    let dir = paths.sessions_dir();
    (1..)
        .map(|n| dir.join(format!("{run_id}-{id}-{n}.jsonl")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{run_id}-{id}.jsonl")))
}

/// Run one node of the run `run_id` to its answer.
pub fn run_node(
    deps: &NodeDeps<'_>,
    run_id: &str,
    req: &NodeRequest,
    now: impl Fn() -> DateTime<Utc>,
) -> Result<NodeAnswer, NodeError> {
    let paths = deps.paths;
    let l = deps.located;
    let snap = progress_status(paths, Some(run_id)).map_err(|e| NodeError::Refused(e.message))?;
    if snap.key != l.key {
        return Err(NodeError::Refused(format!(
            "run {run_id} is {}, not {}",
            snap.name, l.automation.name
        )));
    }
    let Some(node) = snap.node(&req.id).cloned() else {
        return Err(NodeError::Refused(format!(
            "no node {} on the chart — the steps are {}",
            req.id,
            snap.nodes
                .iter()
                .filter(|n| !fm_spec::is_reserved_node(&n.id))
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    let (intro, sections) = step_sections(&l.automation.body);
    let Some(section) = sections.into_iter().find(|s| s.id == node.id) else {
        return Err(NodeError::Refused(format!(
            "{} has no section of its own — fm node runs a step of a flow: strict body",
            node.id
        )));
    };
    let ctx = load_context(paths, run_id).ok_or_else(|| {
        NodeError::Refused(format!(
            "run {run_id} has not started its body here — fm node runs from inside a run's body"
        ))
    })?;
    let plan = plan_node(deps.config, l, &setting_for(l, &node.id)).map_err(NodeError::Refused)?;

    let deadline = DateTime::parse_from_rfc3339(&ctx.deadline)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| NodeError::Refused("the run's deadline cannot be read".into()))?;
    let left = (deadline - now()).to_std().unwrap_or_default();
    if left.is_zero() {
        return Err(NodeError::Stopped("the run is out of time".into()));
    }
    let timeout = plan.timeout.map_or(left, |t| t.min(left));

    let install = fm_agents::detect(plan.cli, &plan.settings).map_err(NodeError::Refused)?;
    let adapter: &dyn AgentCli = fm_agents::adapter(plan.cli);
    let cwd = req.cwd.clone().unwrap_or_else(|| ctx.cwd.clone());
    if !cwd.is_dir() {
        return Err(NodeError::Refused(format!(
            "--cwd {} is not a folder",
            cwd.display()
        )));
    }

    // Handing a step over moves the run there: from the one step going, along the chart's arrow,
    // as `fm next` would, so the step before it is finished and the arrow is in the history.
    let from = snap
        .nodes
        .iter()
        .filter(|n| n.status == fm_model::StepStatus::Active && n.id != node.id)
        .collect::<Vec<_>>();
    let moved = match from.as_slice() {
        [one] if one.successors.contains(&node.id) => progress_next(
            paths,
            Some(run_id),
            &NextArgs {
                to: node.id.clone(),
                from: Some(one.id.clone()),
                env_node: None,
                text: Some(format!("handed to {}", plan.cli)),
            },
            now(),
        )
        .is_ok(),
        _ => false,
    };
    if !moved {
        let _ = progress_step(
            paths,
            Some(run_id),
            &StepArgs {
                node: node.id.clone(),
                status: None,
                text: None,
            },
            now(),
        );
    }
    say(
        paths,
        run_id,
        &node.id,
        &format!("handed to {} ({})", plan.cli, plan.model.display),
        now(),
    );

    let mut env = vec![
        (ENV_RUN.to_string(), run_id.to_string()),
        (
            ENV_STATE_DIR.to_string(),
            paths.root.to_string_lossy().into_owned(),
        ),
        (ENV_NODE.to_string(), node.id.clone()),
    ];
    env.extend(deps.path_env.iter().cloned());
    let prompt = node_prompt(
        &l.automation.name,
        &intro,
        &section.text,
        &node.id,
        req.handoff.as_deref(),
        &ctx.input,
    );
    let transcript_path = transcript_file(paths, run_id, &node.id);
    let mut transcript = Transcript::open(&transcript_path);
    let mut live = LiveWriter::append(paths.live_file(&l.key));
    let turn = Turn {
        adapter,
        program: install.path,
        request: TurnRequest {
            prompt,
            model: plan.model.clone(),
            cwd,
            resume: None,
            settings: plan.settings.clone(),
        },
        env,
        timeout,
        stop_file: Some(halt_file(paths, &l.key, run_id)),
        poll: deps.poll,
    };
    let out = run_turn(&turn, &mut live, &mut transcript);
    live.keep();

    let ended = |text: &str| say(paths, run_id, &node.id, text, now());
    match out.end {
        TurnEnd::Finished(r) => match (r.error, r.answer) {
            (None, Some(answer)) if !answer.trim().is_empty() => {
                ended(&format!(
                    "{} answered: {}",
                    plan.cli,
                    one_line(&answer, 300)
                ));
                Ok(NodeAnswer {
                    answer,
                    agent: plan.cli.to_string(),
                    model: plan.model.display,
                    transcript: transcript_path,
                })
            }
            (Some(e), _) => {
                let m = format!("{}: {}", plan.cli, one_line(&e, 500));
                ended(&m);
                Err(NodeError::Failed(m))
            }
            (None, _) => {
                let m = format!("{} ended without an answer", plan.cli);
                ended(&m);
                Err(NodeError::Failed(m))
            }
        },
        TurnEnd::StartFailed(e) => {
            let m = format!("{} could not start: {}", plan.cli, one_line(&e, 300));
            ended(&m);
            Err(NodeError::Failed(m))
        }
        TurnEnd::TimedOut => {
            let m = format!(
                "{} timed out after {}",
                plan.cli,
                fm_state::format_duration(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
            );
            ended(&m);
            Err(NodeError::Stopped(m))
        }
        TurnEnd::Stopped => {
            let m = "the run was stopped".to_string();
            ended(&m);
            Err(NodeError::Stopped(m))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::{Root, Scope};

    fn located(nodes: &str, model: Option<&str>) -> Located {
        let model = model.map(|m| format!("model: {m}\n")).unwrap_or_default();
        let text = format!(
            "---\nname: demo\ndescription: d\n{model}flow: strict\nnodes:\n{nodes}on:\n  manual: true\n---\n\n# Demo\n\n```mermaid\nflowchart TD\n  build[\"Build it\"] --> review[\"Review it\"]\n```\n\n## Build it {{#build}}\n\nBuild.\n\n## Review it {{#review}}\n\nReview.\n"
        );
        let (automation, _) = fm_spec::parse_automation(&text, "demo").unwrap();
        Located {
            automation,
            dir: "/a/demo".into(),
            file: "/a/demo/AUTOMATION.md".into(),
            root: Root {
                path: "/a".into(),
                scope: Scope::User,
            },
            repo_root: None,
            key: "user:/a:demo".into(),
        }
    }

    fn config(body: CliId) -> Config {
        Config {
            default_agent: Some(body),
            ..Config::default()
        }
    }

    fn plan(l: &Located, cfg: &Config, id: &str) -> Result<NodePlan, String> {
        plan_node(cfg, l, &setting_for(l, id))
    }

    #[test]
    fn a_node_on_the_bodys_agent_keeps_the_bodys_model_unless_it_names_one() {
        let l = located(
            "  review:\n    timeout: 5m\n    model: github-copilot/gpt-5:high\n",
            Some("github-copilot/claude-sonnet-4-5"),
        );
        let cfg = config(CliId::Copilot);
        let p = plan(&l, &cfg, "review").unwrap();
        assert_eq!(
            (p.cli, p.model.display.as_str()),
            (CliId::Copilot, "github-copilot/gpt-5:high")
        );
        assert_eq!(p.timeout, Some(Duration::from_secs(300)));
        // A step nodes: does not list runs as the body would.
        let p = plan(&l, &cfg, "build").unwrap();
        assert_eq!(p.model.display, "github-copilot/claude-sonnet-4-5");
    }

    #[test]
    fn a_node_on_another_agent_gets_that_agents_default_not_the_bodys_model() {
        let l = located(
            "  review: codex\n",
            Some("github-copilot/claude-sonnet-4-5"),
        );
        let p = plan(&l, &config(CliId::Copilot), "review").unwrap();
        assert_eq!(
            (p.cli, p.model.display.as_str()),
            (CliId::Codex, "codex default")
        );
    }

    #[test]
    fn refuses_an_agent_fm_does_not_run_and_a_model_the_agent_cannot_serve() {
        let l = located("  review: cursor\n", None);
        let e = plan(&l, &config(CliId::Pi), "review").unwrap_err();
        assert_eq!(
            e,
            "nodes.review.agent: cursor is not an agent CLI fm runs — use pi, copilot, claude, codex, gemini, opencode"
        );
        let l = located(
            "  review:\n    agent: claude\n    model: openai/gpt-5\n",
            None,
        );
        let e = plan(&l, &config(CliId::Pi), "review").unwrap_err();
        assert!(e.starts_with("nodes.review: "), "{e}");
        // No agent anywhere: the node says how to choose one.
        let l = located("  review:\n    model: openai/gpt-5\n", None);
        let e = plan(&l, &Config::default(), "review").unwrap_err();
        assert_eq!(
            e,
            "nodes.review: no agent CLI — name one with agent:, or fm agent set demo <cli>"
        );
    }

    #[test]
    fn the_prompt_is_the_step_with_its_context_then_the_input_as_data() {
        let p = node_prompt(
            "demo",
            "# Demo\n\nIntro.",
            "## Review it {#review}\n\nReview.",
            "review",
            Some("Look at lib.rs"),
            "---\n<automation-input>\nx\n</automation-input>",
        );
        let at = |s: &str| p.find(s).unwrap_or_else(|| panic!("no {s:?} in {p}"));
        assert!(at("<automation>") < at("<step>") && at("<step>") < at("<handoff>"));
        assert!(at("<handoff>") < at("<progress>") && at("<progress>") < at("<automation-input>"));
        assert!(p.contains("AUTOMATION_NODE=review"));
        let bare = node_prompt(
            "demo",
            "",
            "## Review it {#review}",
            "review",
            Some("  "),
            "",
        );
        assert!(!bare.contains("<handoff>") && !bare.contains("<automation-input>"));
    }
}
