use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};

const SKILL_BODY: &str = include_str!("../../../../claude/skill.md");

#[derive(Args)]
pub struct InstallArgs {
    #[command(subcommand)]
    pub subcommand: InstallSubcommand,
}

#[derive(Subcommand)]
pub enum InstallSubcommand {
    /// Install the xrun skill/instructions for an agent harness
    Skill(InstallSkillArgs),
    /// Install a vendor's Python SDK (`pip install`) into the interpreter xrun's bridge uses
    Sdk(InstallSdkArgs),
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SdkTarget {
    /// `lightning-sdk` (vendor: lightning)
    Lightning,
    /// `google-colab-cli` (vendor: colab)
    Colab,
    /// Both SDKs
    All,
}

#[derive(Args)]
pub struct InstallSdkArgs {
    /// Which SDK to install
    #[arg(value_enum)]
    pub target: SdkTarget,
    /// Print the pip command and exit without running it
    #[arg(long)]
    pub dry_run: bool,
    /// Pass `--upgrade` to pip
    #[arg(long)]
    pub upgrade: bool,
}

#[derive(Args)]
pub struct InstallSkillArgs {
    /// Install Codex project skill files (.agents/skills/xrun + AGENTS.md)
    #[arg(long, conflicts_with = "claude")]
    pub codex: bool,
    /// Install Claude project skill files (.claude/skills/xrun + CLAUDE.md)
    #[arg(long, conflicts_with = "codex")]
    pub claude: bool,
    /// Repository root to install into (defaults to the current directory)
    #[arg(long, value_name = "DIR")]
    pub repo: Option<PathBuf>,
    /// Overwrite existing xrun skill files instead of leaving them unchanged
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: &InstallArgs) -> Result<()> {
    match &args.subcommand {
        InstallSubcommand::Skill(skill_args) => install_skill(skill_args),
        InstallSubcommand::Sdk(sdk_args) => install_sdk(sdk_args),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SdkVendor {
    Lightning,
    Colab,
}

impl SdkVendor {
    fn package(self) -> &'static str {
        match self {
            Self::Lightning => "lightning-sdk",
            Self::Colab => "google-colab-cli",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Lightning => "lightning",
            Self::Colab => "colab",
        }
    }
}

fn install_sdk(args: &InstallSdkArgs) -> Result<()> {
    let vendors: &[SdkVendor] = match args.target {
        SdkTarget::Lightning => &[SdkVendor::Lightning],
        SdkTarget::Colab => &[SdkVendor::Colab],
        SdkTarget::All => &[SdkVendor::Lightning, SdkVendor::Colab],
    };
    let (python, lead) = xrun_core::pybridge::python_argv().ok_or_else(|| {
        anyhow!("python interpreter not found (set XRUN_PYTHON or install Python 3)")
    })?;

    let mut pip_args: Vec<String> = lead;
    pip_args.extend(["-m", "pip", "install"].map(String::from));
    if args.upgrade {
        pip_args.push("--upgrade".into());
    }
    pip_args.extend(vendors.iter().map(|v| v.package().to_string()));

    println!("python: {}", python.display());
    println!("{} {}", python.display(), pip_args.join(" "));
    if args.dry_run {
        return Ok(());
    }

    // Inherited stdio on purpose: the user wants to see pip's output.
    let status = std::process::Command::new(&python)
        .args(&pip_args)
        .status()
        .with_context(|| format!("failed to run {}", python.display()))?;
    if !status.success() {
        bail!("pip install failed ({status})");
    }

    let mut failed = false;
    for v in vendors {
        match ping_sdk(*v) {
            Ok(version) => println!("{}: {} {version} OK", v.name(), v.package()),
            Err(e) => {
                failed = true;
                eprintln!("{}: installed, but the bridge ping failed: {e}", v.name());
            }
        }
    }
    if failed {
        bail!("SDK installed but not importable by the bridge");
    }
    Ok(())
}

fn ping_sdk(v: SdkVendor) -> Result<String, String> {
    match v {
        SdkVendor::Lightning => {
            use xrun_lightning::{LightningBridge, PyLightningBridge};
            let bridge = PyLightningBridge::new(&Default::default());
            bridge
                .ping()
                .map(|i| i.sdk_version)
                .map_err(|e| e.to_string())
        }
        SdkVendor::Colab => {
            use xrun_colab::{ColabBridge, PyColabBridge};
            PyColabBridge::new()
                .ping()
                .map(|i| i.sdk_version)
                .map_err(|e| e.to_string())
        }
    }
}

fn install_skill(args: &InstallSkillArgs) -> Result<()> {
    let harness = match (args.codex, args.claude) {
        (true, false) => Harness::Codex,
        (false, true) => Harness::Claude,
        (false, false) => bail!("choose a harness: `xrun install skill --codex` or `--claude`"),
        (true, true) => unreachable!("clap conflicts_with prevents selecting both harnesses"),
    };

    let repo = args
        .repo
        .clone()
        .unwrap_or(std::env::current_dir().context("failed to read current directory")?);
    if !repo.is_dir() {
        bail!("repo path is not a directory: {}", repo.display());
    }

    let skill_path = repo.join(harness.skill_path());
    write_skill(&skill_path, args.force)?;

    let instruction_path = repo.join(harness.instruction_file());
    upsert_instruction_pointer(&instruction_path, harness)?;

    println!("installed xrun {} skill:", harness.name());
    println!("  {}", skill_path.display());
    println!("  {}", instruction_path.display());

    Ok(())
}

fn write_skill(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        println!(
            "kept existing skill at {} (pass --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("skill path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(path, SKILL_BODY).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn upsert_instruction_pointer(path: &Path, harness: Harness) -> Result<()> {
    let block = harness.instruction_block();
    let mut content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };

    if let Some(start) = content.find("<!-- xrun-skill -->") {
        if let Some(end) = content[start..].find("<!-- /xrun-skill -->") {
            content.replace_range(start..start + end + "<!-- /xrun-skill -->".len(), block);
        } else {
            // Migrate only the exact legacy block; preserve unrelated user text.
            let legacy = block
                .replace(".agents/skills/", ".codex/skills/")
                .replace("\n<!-- /xrun-skill -->", "");
            if content.contains(&legacy) {
                content = content.replacen(&legacy, block, 1);
            } else {
                bail!("unrecognized xrun instruction block in {}; preserve it and update the skill pointer manually", path.display());
            }
        }
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(());
    }

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(block);
    content.push('\n');

    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Harness {
    Codex,
    Claude,
}

impl Harness {
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    fn skill_path(self) -> &'static str {
        match self {
            Self::Codex => ".agents/skills/xrun/SKILL.md",
            Self::Claude => ".claude/skills/xrun/SKILL.md",
        }
    }

    fn instruction_file(self) -> &'static str {
        match self {
            Self::Codex => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
        }
    }

    fn instruction_block(self) -> &'static str {
        match self {
            Self::Codex => {
                "<!-- xrun-skill -->\n# xrun Skill\n\nWhen working with ML experiment runs in this repository, use the project skill at `.agents/skills/xrun/SKILL.md`.\n<!-- /xrun-skill -->"
            }
            Self::Claude => {
                "<!-- xrun-skill -->\n# xrun Skill\n\nWhen working with ML experiment runs in this repository, use the project skill at `.claude/skills/xrun/SKILL.md`.\n<!-- /xrun-skill -->"
            }
        }
    }
}
