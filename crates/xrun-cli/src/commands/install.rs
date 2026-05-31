use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};

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
}

#[derive(Args)]
pub struct InstallSkillArgs {
    /// Install Codex project skill files (.codex/skills/xrun + AGENTS.md)
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

    if content.contains("<!-- xrun-skill -->") {
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
            Self::Codex => ".codex/skills/xrun/SKILL.md",
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
                "<!-- xrun-skill -->\n# xrun Skill\n\nWhen working with ML experiment runs in this repository, use the project skill at `.codex/skills/xrun/SKILL.md`."
            }
            Self::Claude => {
                "<!-- xrun-skill -->\n# xrun Skill\n\nWhen working with ML experiment runs in this repository, use the project skill at `.claude/skills/xrun/SKILL.md`."
            }
        }
    }
}
