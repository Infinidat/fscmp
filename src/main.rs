#[macro_use]
extern crate clap;
#[macro_use]
extern crate log;
extern crate rayon;
extern crate simplelog;

mod cmp;
mod file_ext_exact;

use clap::{App, Arg};
use cmp::{Comparison, FSCmp};
use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use std::process;

fn run() -> Result<Comparison, std::io::Error> {
    let matches = App::new("fscmp")
        .version(crate_version!())
        .arg(Arg::with_name("first").required(true))
        .arg(Arg::with_name("second").required(true))
        .arg(
            Arg::with_name("log-dir")
                .long("log-dir")
                .takes_value(true)
                .value_name("LOG_DIR")
                .validator_os(|log_dir| {
                    if Path::new(log_dir).is_dir() {
                        Ok(())
                    } else {
                        Err("Log directory does not exist".into())
                    }
                })
                .help("Directory to store log(s) in"),
        )
        .arg(
            Arg::with_name("content-size")
                .long("content-size")
                .takes_value(true)
                .value_name("SIZE")
                .help("Compare arguments using specified size (used for block devices)"),
        )
        .arg(
            Arg::with_name("full-compare-limit")
                .long("full-compare-limit")
                .takes_value(true)
                .value_name("SIZE")
                .help("Size in bytes to limit full compare (larger files will be sampled)"),
        )
        .arg(
            Arg::with_name("ignored-dirs")
                .long("ignore-dir")
                .takes_value(true)
                .value_name("DIR")
                .multiple(true)
                .help("Directories to ignore when comparing"),
        )
        .get_matches();

    if let Some(log_dir) = matches.value_of_os("log-dir") {
        let log_dir = Path::new(log_dir);
        simplelog::WriteLogger::init(
            simplelog::LevelFilter::max(),
            simplelog::Config {
                time_format: Some("%F %T%.3f"),
                ..Default::default()
            },
            File::create(log_dir.join(format!(
                "{}.{}.log",
                env!("CARGO_PKG_NAME"),
                process::id()
            )))?,
        ).unwrap();
    }

    let content_size = if matches.is_present("content-size") {
        Some(value_t!(matches, "content-size", u64).unwrap_or_else(|e| e.exit()))
    } else {
        None
    };

    let full_compare_limit = if matches.is_present("full-compare-limit") {
        Some(value_t!(matches, "full-compare-limit", u64).unwrap_or_else(|e| e.exit()))
    } else {
        None
    };

    let ignored_dirs = matches
        .values_of_os("ignored-dirs")
        .map(|v| v.into_iter().map(|s| s.into()).collect())
        .unwrap_or_else(HashSet::new);

    let fscmp = FSCmp::new(
        matches.value_of_os("first").unwrap().into(),
        matches.value_of_os("second").unwrap().into(),
        full_compare_limit,
        ignored_dirs,
    );

    rayon::ThreadPoolBuilder::new()
        .stack_size(8 * 1024 * 1024)
        .build_global()
        .unwrap();

    Ok(if let Some(content_size) = content_size {
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
            debug!("Error: {}", e);
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
