use crate::state::{AppState, Screen};
use xrun_core::RunId;

pub enum PaletteAction {
    Quit,
    GotoScreen(Screen),
    ShowLaunchConfirm(String),
    ShowStopConfirm(Option<RunId>, String),
    ShowPullConfirm(Option<RunId>, String),
    OpenRunDetail(String),
    ApplyTheme(String),
    Rerun(Option<RunId>, String),
    Nothing,
}

const COMMANDS: &[&str] = &[
    "launch ",
    "stop ",
    "pull ",
    "rerun ",
    "show ",
    "goto runs",
    "goto instances",
    "goto settings",
    "theme default",
    "theme high_contrast",
    "quit",
];

pub fn compute_completions(input: &str) -> Vec<String> {
    let input_lower = input.to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&input_lower))
        .map(|c| c.to_string())
        .collect()
}

pub fn parse_command(cmd: &str, state: &AppState) -> PaletteAction {
    let cmd = cmd.trim();
    if cmd == "quit" {
        return PaletteAction::Quit;
    }
    if let Some(dest) = cmd.strip_prefix("goto ") {
        return match dest.trim() {
            "runs" => PaletteAction::GotoScreen(Screen::Runs),
            "instances" => PaletteAction::GotoScreen(Screen::Instances),
            "settings" => PaletteAction::GotoScreen(Screen::Settings),
            _ => PaletteAction::Nothing,
        };
    }
    if let Some(path) = cmd.strip_prefix("launch ") {
        return PaletteAction::ShowLaunchConfirm(path.trim().to_string());
    }
    if let Some(id_str) = cmd.strip_prefix("stop ") {
        let id_str = id_str.trim();
        let run_id = find_run_id(state, id_str);
        return PaletteAction::ShowStopConfirm(run_id, id_str.to_string());
    }
    if let Some(id_str) = cmd.strip_prefix("pull ") {
        let id_str = id_str.trim();
        let run_id = find_run_id(state, id_str);
        return PaletteAction::ShowPullConfirm(run_id, id_str.to_string());
    }
    if let Some(id_str) = cmd.strip_prefix("show ") {
        return PaletteAction::OpenRunDetail(id_str.trim().to_string());
    }
    if let Some(id_str) = cmd.strip_prefix("rerun ") {
        let id_str = id_str.trim();
        let run_id = find_run_id(state, id_str);
        return PaletteAction::Rerun(run_id, id_str.to_string());
    }
    if let Some(theme) = cmd.strip_prefix("theme ") {
        return PaletteAction::ApplyTheme(theme.trim().to_string());
    }
    PaletteAction::Nothing
}

fn find_run_id(state: &AppState, id_str: &str) -> Option<RunId> {
    state
        .runs
        .active_runs
        .iter()
        .chain(state.runs.recent_runs.iter())
        .find(|r| r.id.to_string() == id_str || r.name == id_str)
        .map(|r| r.id.clone())
}
