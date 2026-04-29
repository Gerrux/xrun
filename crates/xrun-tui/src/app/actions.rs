use anyhow::Result;
use xrun_core::{Manifest, RunStatus};

use crate::screens::instances::InstancesAction;
use crate::screens::launch::LaunchAction;
use crate::screens::run_detail::RunDetailAction;
use crate::screens::runs::RunsAction;
use crate::screens::settings::SettingsAction;
use crate::state::{ConfirmAction, Modal, Screen, Tab};
use crate::theme::Theme;

use super::App;

impl App {
    pub(super) fn handle_runs_action(&mut self, action: RunsAction) -> Result<bool> {
        match action {
            RunsAction::OpenRunDetail(id) => {
                self.load_run_detail(&id)?;
                self.state.push_screen(Screen::RunDetail(id, Tab::Stages));
            }
            RunsAction::OpenLaunch => {
                self.load_launch_manifests()?;
                self.state.push_screen(Screen::Launch);
            }
            RunsAction::OpenInstances => {
                self.load_instances()?;
                self.state.push_screen(Screen::Instances);
            }
            RunsAction::OpenSettings => {
                self.load_settings();
                self.state.push_screen(Screen::Settings);
            }
            RunsAction::OpenVendors => {
                self.state.push_screen(Screen::Vendors);
                self.trigger_probe(None);
            }
            RunsAction::ShowStopConfirm(id, name) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Stop run '{}'?", name),
                    action: ConfirmAction::StopRun(id),
                });
                self.state.dirty = true;
            }
            RunsAction::ShowPullConfirm(id, name) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Pull best checkpoint for '{}'?", name),
                    action: ConfirmAction::PullRun(id),
                });
                self.state.dirty = true;
            }
            RunsAction::Rerun(id) => {
                self.handle_rerun(id)?;
            }
            RunsAction::Quit => return Ok(true),
            RunsAction::Nothing => {}
        }
        Ok(false)
    }

    pub(super) fn handle_run_detail_action(&mut self, action: RunDetailAction) -> Result<bool> {
        match action {
            RunDetailAction::Back => {
                self.state.pop_screen();
            }
            RunDetailAction::SwitchTab(tab) => {
                if let Screen::RunDetail(id, _) = &self.state.screen {
                    let id = id.clone();
                    self.state.screen = Screen::RunDetail(id, tab);
                    self.state.dirty = true;
                }
            }
            RunDetailAction::OpenEditor(path) => {
                self.state.editor_path = Some(path);
            }
            RunDetailAction::ToggleAutoscroll => {
                self.state.run_detail.log.autoscroll = !self.state.run_detail.log.autoscroll;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollUp => {
                self.state.run_detail.log.scroll =
                    self.state.run_detail.log.scroll.saturating_sub(1);
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollDown => {
                self.state.run_detail.log.scroll =
                    self.state.run_detail.log.scroll.saturating_add(1);
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollTop => {
                self.state.run_detail.log.scroll = 0;
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollBottom => {
                self.state.run_detail.log.scroll = usize::MAX;
                self.state.run_detail.log.autoscroll = true;
                self.state.dirty = true;
            }
            RunDetailAction::Nothing => {}
        }
        Ok(false)
    }

    pub(super) fn handle_launch_action(&mut self, action: LaunchAction) -> Result<bool> {
        match action {
            LaunchAction::Confirm(path) => {
                let estimate = read_manifest_estimate(&path);
                let message = match estimate {
                    Some((vendor, hourly, max_hours, max_cost)) => {
                        let projected = match (max_hours, max_cost) {
                            (Some(h), Some(c)) => Some((h * hourly).min(c)),
                            (Some(h), None) => Some(h * hourly),
                            (None, Some(c)) => Some(c),
                            (None, None) => None,
                        };
                        let proj_str = match projected {
                            Some(p) => format!(" \u{2192} max ${:.2}", p),
                            None => " \u{2192} no cap".to_string(),
                        };
                        format!(
                            "Launch '{}'?\n{} \u{00b7} ${:.4}/hr{}",
                            path, vendor, hourly, proj_str
                        )
                    }
                    None => format!("Launch manifest '{}'?", path),
                };
                self.state.modal = Some(Modal::Confirm {
                    message,
                    action: ConfirmAction::LaunchRun(path),
                });
                self.state.dirty = true;
            }
            LaunchAction::Back => {
                self.state.pop_screen();
            }
            LaunchAction::Nothing => {}
        }
        Ok(false)
    }

    pub(super) fn handle_instances_action(&mut self, action: InstancesAction) -> Result<bool> {
        match action {
            InstancesAction::ShowDestroyConfirm(id) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Destroy orphan instance '{}'?", id),
                    action: ConfirmAction::DestroyInstance(id),
                });
                self.state.dirty = true;
            }
            InstancesAction::Back => {
                self.state.pop_screen();
            }
            InstancesAction::Nothing => {}
        }
        Ok(false)
    }

    pub(super) fn handle_settings_action(&mut self, action: SettingsAction) -> Result<bool> {
        match action {
            SettingsAction::SaveTheme(name) => {
                self.config.tui.theme = name.clone();
                self.state.theme = Theme::from_name(&name);
                self.state.settings.theme = name;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalActive(v) => {
                self.config.poller.interval_active_secs = v;
                self.state.settings.poll_interval_active = v;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalIdle(v) => {
                self.config.poller.interval_idle_secs = v;
                self.state.settings.poll_interval_idle = v;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SaveDefaultVendor(vendor) => {
                let trimmed = vendor.as_deref().map(str::trim).unwrap_or("");
                let parsed = match trimmed.to_ascii_lowercase().as_str() {
                    "" => Some(None),
                    "vast" => Some(Some(xrun_core::manifest::types::Vendor::Vast)),
                    "kaggle" => Some(Some(xrun_core::manifest::types::Vendor::Kaggle)),
                    _ => None,
                };
                if let Some(v) = parsed {
                    self.config.defaults.vendor = v;
                    self.state.settings.default_vendor = trimmed.to_ascii_lowercase();
                    self.save_config();
                } else {
                    tracing::warn!("ignoring unknown vendor '{}'", trimmed);
                }
                self.state.dirty = true;
            }
            SettingsAction::Back => {
                self.state.pop_screen();
            }
            SettingsAction::Nothing => {}
        }
        Ok(false)
    }

    pub(super) fn execute_confirm_action(&mut self, action: ConfirmAction) -> Result<()> {
        match action {
            ConfirmAction::StopRun(id) => {
                self.store.update_run_status(&id, RunStatus::Cancelled)?;
                self.reload_runs()?;
            }
            ConfirmAction::PullRun(id) => {
                tracing::info!("pull requested for run {}", id);
            }
            ConfirmAction::DestroyInstance(instance_id) => {
                self.store
                    .update_instance_destroyed(&instance_id, chrono::Utc::now())?;
                self.load_instances()?;
            }
            ConfirmAction::LaunchRun(path) => {
                tracing::info!("launch requested for manifest {}", path);
                self.state.pop_screen();
                self.reload_runs()?;
            }
            ConfirmAction::RevokeVendor(name) => {
                self.revoke_vendor(&name)?;
            }
        }
        Ok(())
    }
}

/// Best-effort budget summary from a manifest path: (vendor, $/hr, max_hours,
/// max_cost). Returns `None` if the manifest is unreadable or not a vast spec —
/// the caller falls back to a plain confirm message.
fn read_manifest_estimate(
    path: &str,
) -> Option<(String, f64, Option<f64>, Option<f64>)> {
    let yaml = std::fs::read_to_string(path).ok()?;
    let manifest = Manifest::from_yaml_str(&yaml).ok()?;
    let vast = manifest.vast.as_ref()?;
    let hourly = vast.price.as_ref().map(|p| p.max_per_hour).unwrap_or(0.0);
    Some(("Vast.ai".to_string(), hourly, None, None))
}
