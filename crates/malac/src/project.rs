//! `malac index` and `malac context`: the project index and what a session
//! in this directory sees.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::Args;
use malac_core::Runtime;
use malac_core::config::{self, Config};
use malac_core::context::index::Index;

use crate::repl::working_dir;

#[derive(Args)]
pub struct IndexArgs {
    /// Regenerate from scratch instead of reading only changed files.
    #[arg(long)]
    rebuild: bool,
    /// Print the index afterwards.
    #[arg(long)]
    print: bool,
    #[arg(long)]
    cwd: Option<PathBuf>,
}

#[derive(Args)]
pub struct ContextArgs {
    #[arg(long)]
    json: bool,
    #[arg(long)]
    cwd: Option<PathBuf>,
}

pub fn index(args: IndexArgs) -> Result<()> {
    let cwd = working_dir(args.cwd.as_ref())?;
    let loaded = Config::load(&cwd, None)?;
    if !loaded.config.context.index {
        bail!("the project index is off (`[context] index = false`)");
    }
    let root = config::project_root(&cwd);
    let index = Index::new(&root, loaded.config.context.index_max_files);
    let started = Instant::now();
    let refreshed = index.refresh(args.rebuild)?;
    if args.print {
        print!("{}", index.read()?);
        return Ok(());
    }
    println!(
        "{} {} files ({} read) in {} ms: {}",
        if refreshed.written {
            "Indexed"
        } else {
            "Up to date:"
        },
        refreshed.files,
        refreshed.parsed,
        started.elapsed().as_millis(),
        index.path().display()
    );
    Ok(())
}

pub fn context(args: ContextArgs) -> Result<()> {
    let cwd = working_dir(args.cwd.as_ref())?;
    let loaded = Config::load(&cwd, None)?;
    for warning in &loaded.warnings {
        eprintln!("! {warning}");
    }
    let runtime = Runtime::new(loaded.config, cwd)?;
    let report = runtime.context_report();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", report.render());
    }
    Ok(())
}
