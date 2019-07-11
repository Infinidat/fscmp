mod cmp;

use crate::cmp::{Comparison, FSCmp};
use log::error;
use std::collections::HashSet;
#[cfg(feature = "simplelog")]
use std::ffi::{OsStr, OsString};
#[cfg(feature = "simplelog")]
use std::fs::File;
use std::iter::FromIterator;
#[cfg(feature = "simplelog")]
use std::path::Path;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;

#[cfg(feature = "simplelog")]
fn parse_log_dir(src: &OsStr) -> Result<PathBuf, OsString> {
    let path = Path::new(src);
    if path.is_dir() {
        Ok(path.into())
    } else {
        Err("Log directory does not exist".into())
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "fscmp")]
/// Directory/file comparison utility
struct Opt {
    #[cfg(feature = "simplelog")]
    #[structopt(name = "log-dir", long, parse(try_from_os_str = "parse_log_dir"))]
    /// Directory to store log(s) in
    log_dir: Option<PathBuf>,

    #[structopt(name = "content-size", long)]
    /// Compare arguments using specified size (used for block devices)
    content_size: Option<u64>,

    #[structopt(name = "full-compare-limit", long)]
    /// Size in bytes to limit full compare (larger files will be sampled)
    full_compare_limit: Option<u64>,

    #[structopt(name = "ignore-dir", long, raw(number_of_values = "1"))]
    /// Directories to ignore when comparing
    ignored_dirs: Vec<PathBuf>,

    #[structopt(parse(from_os_str), raw(required = "true"))]
    first: PathBuf,

    #[structopt(parse(from_os_str), raw(required = "true"))]
    second: PathBuf,
}

fn run() -> failure::Fallible<Comparison> {
    let opt = Opt::from_args();

    #[cfg(feature = "loggest")]
    let mut _flush_log = loggest::init(
        log::LevelFilter::max(),
        format!("{}.{}.log", env!("CARGO_PKG_NAME"), process::id()),
    )
    .unwrap();

    #[cfg(feature = "simplelog")]
    {
        if let Some(log_dir) = opt.log_dir {
            let log_file = log_dir.join(format!("{}.{}.log", env!("CARGO_PKG_NAME"), process::id()));
            simplelog::WriteLogger::init(
                simplelog::LevelFilter::max(),
                simplelog::Config {
                    target: None,
                    time_format: Some("%F %T%.3f"),
                    ..Default::default()
                },
                File::create(log_file)?,
            )
            .unwrap();
        }
    }

    let fscmp = FSCmp::new(
        opt.first,
        opt.second,
        opt.full_compare_limit,
        HashSet::from_iter(opt.ignored_dirs.into_iter()),
    );

    Ok(if let Some(content_size) = opt.content_size {
        fscmp.contents(content_size)?
    } else {
        fscmp.dirs()?
    })
}

fn main() {
    match run() {
        Ok(Comparison::Equal) => (),
        Ok(comp) => {
            eprintln!("{}", comp);
            std::process::exit(1);
        }
        Err(e) => {
            error!("Error: {}", e);
            eprintln!("Error: {}", e);
            std::process::exit(2);
        }
    }
}
