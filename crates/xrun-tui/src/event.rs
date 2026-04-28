use crossterm::event::KeyEvent;
pub use xrun_core::DataUpdate;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    DataUpdate(DataUpdate),
    Quit,
}
