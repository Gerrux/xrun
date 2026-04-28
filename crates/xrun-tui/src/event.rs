use crossterm::event::KeyEvent;
use xrun_core::RunId;

#[derive(Debug, Clone)]
pub enum DataUpdate {
    RunCreated(RunId),
    RunStatusChanged(RunId),
    EventsAppended(RunId, usize),
    MetricsAppended(RunId, usize),
    LogsAppended(RunId, u64),
}

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    DataUpdate(DataUpdate),
    Quit,
}
