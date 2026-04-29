#![deny(unsafe_code)]

//! Billable-action confirm prompts. CLI side of the budget guard.
//!
//! TUI has a parallel implementation in `xrun-tui::view::confirm_billable`.
//! Both consume the same `ConfirmEstimate` and apply the same risk-tier rules.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{bail, Result};
use xrun_core::config::BudgetConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    /// Below `require_confirm_above_hourly` — no prompt.
    Free,
    /// Between the two thresholds — y/N prompt.
    Confirm,
    /// Above `require_typed_confirm_above_hourly` — must type `launch`.
    TypedConfirm,
}

impl RiskTier {
    pub fn classify(hourly: f64, cfg: &BudgetConfig) -> Self {
        if hourly >= cfg.require_typed_confirm_above_hourly {
            RiskTier::TypedConfirm
        } else if hourly >= cfg.require_confirm_above_hourly {
            RiskTier::Confirm
        } else {
            RiskTier::Free
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfirmEstimate {
    pub vendor: String,
    pub gpu: String,
    pub hourly_usd: f64,
    pub max_hours: Option<f64>,
    pub max_cost_usd: Option<f64>,
    pub balance_usd: Option<f64>,
}

impl ConfirmEstimate {
    /// Worst-case spend if the run hits the lifetime cap. Falls back to a
    /// configured cost cap when no lifetime cap is set.
    pub fn projected_max(&self) -> Option<f64> {
        match (self.max_hours, self.max_cost_usd) {
            (Some(h), Some(c)) => Some((h * self.hourly_usd).min(c)),
            (Some(h), None) => Some(h * self.hourly_usd),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        }
    }
}

/// Prompt the user to confirm a billable action. Honors `--yes` for scripts and
/// fails loudly when stdin is not a TTY without `--yes` (so a piped `xrun launch`
/// can never start an instance unattended).
pub fn confirm_billable_or_exit(
    estimate: &ConfirmEstimate,
    cfg: &BudgetConfig,
    yes_flag: bool,
) -> Result<()> {
    let tier = RiskTier::classify(estimate.hourly_usd, cfg);

    if matches!(tier, RiskTier::Free) {
        return Ok(());
    }

    if yes_flag {
        eprintln!(
            "[budget] confirm bypassed via --yes ({})",
            describe_tier(tier)
        );
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        bail!(
            "billable action requires confirmation ({}). \
             Re-run with --yes to skip the prompt in non-interactive contexts.",
            describe_tier(tier)
        );
    }

    print_summary(estimate, tier);

    let stdin = io::stdin();
    let mut line = String::new();
    match tier {
        RiskTier::Confirm => {
            print!("Confirm? [y/N]: ");
            io::stdout().flush().ok();
            line.clear();
            stdin.lock().read_line(&mut line)?;
            let trimmed = line.trim().to_lowercase();
            if trimmed != "y" && trimmed != "yes" {
                bail!("aborted by user");
            }
        }
        RiskTier::TypedConfirm => {
            print!("This is a high-cost run. Type 'launch' to confirm: ");
            io::stdout().flush().ok();
            line.clear();
            stdin.lock().read_line(&mut line)?;
            if line.trim() != "launch" {
                bail!("aborted by user (typed confirmation did not match)");
            }
        }
        RiskTier::Free => unreachable!(),
    }
    Ok(())
}

fn describe_tier(tier: RiskTier) -> &'static str {
    match tier {
        RiskTier::Free => "free",
        RiskTier::Confirm => "y/N tier",
        RiskTier::TypedConfirm => "typed-confirm tier",
    }
}

fn print_summary(estimate: &ConfirmEstimate, tier: RiskTier) {
    eprintln!();
    eprintln!(
        "  {} · {} · ${:.4}/hr",
        estimate.vendor, estimate.gpu, estimate.hourly_usd
    );
    if let Some(max) = estimate.projected_max() {
        let cap_desc = match (estimate.max_hours, estimate.max_cost_usd) {
            (Some(h), _) => format!("{h:.1}h"),
            (None, Some(c)) => format!("${c:.2} cap"),
            _ => "—".to_string(),
        };
        eprintln!("  Cap: {cap_desc} → ${max:.2} max");
    } else {
        eprintln!("  Cap: none configured (instance can run indefinitely)");
    }
    if let Some(bal) = estimate.balance_usd {
        let runway = if estimate.hourly_usd > 0.0 {
            format!("~{:.1}h runway", bal / estimate.hourly_usd)
        } else {
            "—".to_string()
        };
        eprintln!("  Balance: ${bal:.2}   →   {runway}");
    }
    if matches!(tier, RiskTier::TypedConfirm) {
        eprintln!("  /!\\ high-cost tier");
    }
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_tier_classification() {
        let cfg = BudgetConfig::default();
        assert_eq!(RiskTier::classify(0.10, &cfg), RiskTier::Free);
        assert_eq!(RiskTier::classify(0.50, &cfg), RiskTier::Confirm);
        assert_eq!(RiskTier::classify(1.99, &cfg), RiskTier::Confirm);
        assert_eq!(RiskTier::classify(2.00, &cfg), RiskTier::TypedConfirm);
        assert_eq!(RiskTier::classify(99.0, &cfg), RiskTier::TypedConfirm);
    }

    #[test]
    fn projected_max_picks_min_of_caps() {
        let est = ConfirmEstimate {
            vendor: "vast".into(),
            gpu: "RTX 4090".into(),
            hourly_usd: 1.0,
            max_hours: Some(8.0),
            max_cost_usd: Some(5.0),
            balance_usd: None,
        };
        // 8h * $1/hr = $8, but max_cost = $5 — take the smaller.
        assert_eq!(est.projected_max(), Some(5.0));
    }

    #[test]
    fn projected_max_when_no_caps() {
        let est = ConfirmEstimate {
            vendor: "vast".into(),
            gpu: "RTX 4090".into(),
            hourly_usd: 1.0,
            max_hours: None,
            max_cost_usd: None,
            balance_usd: None,
        };
        assert_eq!(est.projected_max(), None);
    }
}
