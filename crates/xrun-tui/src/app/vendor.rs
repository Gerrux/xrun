use std::sync::atomic::Ordering;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use xrun_core::config::Credentials;

use crate::screens::vendors::VendorsAction;
use crate::state::{ConfirmAction, EditField, Modal};

use super::App;

impl App {
    pub(super) fn handle_vendors_action(&mut self, action: VendorsAction) -> Result<bool> {
        match action {
            VendorsAction::Back => {
                self.state.pop_screen();
            }
            VendorsAction::OpenEdit(vendor) => {
                self.open_vendor_edit(&vendor);
            }
            VendorsAction::ImportNative(vendor) => {
                self.import_native_vendor(&vendor);
            }
            VendorsAction::TestConnection(vendor) => {
                self.trigger_probe(Some(&vendor));
                self.state.vendors.flash = Some(format!("probing {}...", vendor));
                self.state.dirty = true;
            }
            VendorsAction::ShowRevokeConfirm(vendor) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Revoke credentials for '{}'?", vendor),
                    action: ConfirmAction::RevokeVendor(vendor),
                });
                self.state.dirty = true;
            }
            VendorsAction::Nothing => {}
        }
        Ok(false)
    }

    fn open_vendor_edit(&mut self, vendor: &str) {
        let fields = match vendor {
            "vast" => vec![EditField {
                label: "api_key".to_string(),
                value: self
                    .state
                    .credentials
                    .vast
                    .api_key
                    .clone()
                    .unwrap_or_default(),
                secret: true,
            }],
            "kaggle" => vec![
                EditField {
                    label: "username".to_string(),
                    value: self
                        .state
                        .credentials
                        .kaggle
                        .username
                        .clone()
                        .unwrap_or_default(),
                    secret: false,
                },
                EditField {
                    label: "key".to_string(),
                    value: self
                        .state
                        .credentials
                        .kaggle
                        .key
                        .clone()
                        .unwrap_or_default(),
                    secret: true,
                },
            ],
            "mlflow" => vec![
                EditField {
                    label: "url".to_string(),
                    value: self.config.mlflow.url.clone().unwrap_or_default(),
                    secret: false,
                },
                EditField {
                    label: "token".to_string(),
                    value: self
                        .state
                        .credentials
                        .mlflow
                        .token
                        .clone()
                        .unwrap_or_default(),
                    secret: true,
                },
            ],
            _ => return,
        };
        self.state.modal = Some(Modal::VendorEdit {
            vendor: vendor.to_string(),
            fields,
            focus: 0,
            flash: None,
        });
        self.state.dirty = true;
    }

    fn import_native_vendor(&mut self, vendor: &str) {
        let result: Result<String, String> = match vendor {
            "vast" => match Credentials::import_vast_native() {
                Ok(Some(token)) => {
                    self.state.credentials.vast.api_key = Some(token);
                    Ok("imported vast api_key from ~/.config/vastai/vast_api_key".to_string())
                }
                Ok(None) => Err("native vast key file not found or empty".to_string()),
                Err(e) => Err(format!("read failed: {}", e)),
            },
            "kaggle" => match Credentials::import_kaggle_native() {
                Ok(Some((u, k))) => {
                    self.state.credentials.kaggle.username = Some(u);
                    self.state.credentials.kaggle.key = Some(k);
                    Ok("imported kaggle.json".to_string())
                }
                Ok(None) => Err("kaggle.json not found or missing fields".to_string()),
                Err(e) => Err(format!("read failed: {}", e)),
            },
            other => Err(format!("no native import for vendor '{}'", other)),
        };

        match result {
            Ok(msg) => {
                if let Err(e) = self.persist_credentials() {
                    self.state.vendors.flash = Some(format!("save failed: {}", e));
                } else {
                    self.state.vendors.flash = Some(msg);
                    self.refresh_vendor_probe();
                    self.trigger_probe(Some(vendor));
                }
            }
            Err(e) => {
                self.state.vendors.flash = Some(e);
            }
        }
        self.state.dirty = true;
    }

    pub(super) fn revoke_vendor(&mut self, vendor: &str) -> Result<()> {
        match vendor {
            "vast" => self.state.credentials.vast.api_key = None,
            "kaggle" => {
                self.state.credentials.kaggle.username = None;
                self.state.credentials.kaggle.key = None;
            }
            "mlflow" => self.state.credentials.mlflow.token = None,
            _ => {}
        }
        self.persist_credentials().map_err(anyhow::Error::from)?;
        self.state.vendor_statuses.remove(vendor);
        self.state.vendors.flash = Some(format!("revoked {}", vendor));
        self.refresh_vendor_probe();
        self.state.dirty = true;
        Ok(())
    }

    pub(super) fn persist_credentials(&self) -> std::io::Result<()> {
        let Some(dir) = &self.config_dir else {
            return Ok(());
        };
        self.state
            .credentials
            .save(dir)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub(super) fn refresh_vendor_probe(&mut self) {
        if let Some(flag) = &self.probe_shutdown {
            flag.store(true, Ordering::Relaxed);
        }
        self.probe_shutdown = None;
        self.probe_tx = None;
        self.start_vendor_probe();
    }

    pub(super) fn handle_vendor_edit_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                if let Some(Modal::VendorEdit { fields, .. }) = self.state.modal.as_mut() {
                    // wipe secrets from in-memory modal state on close
                    for f in fields.iter_mut() {
                        if f.secret {
                            f.value.clear();
                        }
                    }
                }
                self.state.modal = None;
                self.state.dirty = true;
            }
            KeyCode::Tab | KeyCode::Down => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if !fields.is_empty() {
                        *focus = (*focus + 1) % fields.len();
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if !fields.is_empty() {
                        *focus = if *focus == 0 {
                            fields.len() - 1
                        } else {
                            *focus - 1
                        };
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.value.pop();
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.value.push(c);
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Enter => {
                self.commit_vendor_edit()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn commit_vendor_edit(&mut self) -> Result<()> {
        let Some(Modal::VendorEdit { vendor, fields, .. }) = self.state.modal.take() else {
            return Ok(());
        };
        match vendor.as_str() {
            "vast" => {
                let key = fields
                    .iter()
                    .find(|f| f.label == "api_key")
                    .map(|f| f.value.clone());
                self.state.credentials.vast.api_key = key.filter(|s| !s.is_empty());
            }
            "kaggle" => {
                let user = fields
                    .iter()
                    .find(|f| f.label == "username")
                    .map(|f| f.value.clone());
                let key = fields
                    .iter()
                    .find(|f| f.label == "key")
                    .map(|f| f.value.clone());
                self.state.credentials.kaggle.username = user.filter(|s| !s.is_empty());
                self.state.credentials.kaggle.key = key.filter(|s| !s.is_empty());
            }
            "mlflow" => {
                let url = fields
                    .iter()
                    .find(|f| f.label == "url")
                    .map(|f| f.value.clone());
                let token = fields
                    .iter()
                    .find(|f| f.label == "token")
                    .map(|f| f.value.clone());
                self.config.mlflow.url = url.filter(|s| !s.is_empty());
                self.state.credentials.mlflow.token = token.filter(|s| !s.is_empty());
                self.save_config();
            }
            _ => {}
        }
        if let Err(e) = self.persist_credentials() {
            self.state.vendors.flash = Some(format!("save failed: {}", e));
        } else {
            self.state.vendors.flash = Some(format!("saved {} credentials", vendor));
            self.refresh_vendor_probe();
            self.trigger_probe(Some(&vendor));
        }
        self.state.dirty = true;
        Ok(())
    }
}
