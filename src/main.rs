#[macro_use]
extern crate clap;
#[macro_use]
extern crate getset;

mod cmp;
mod config;
mod file_ext_exact;
mod range_chunks;

use clap::{App, Arg};
use cmp::{Comparison, EntryInfo};
use std::collections::HashSet;

fn run() -> Result<bool, std::io::Error> {
    let matches = App::new("fscmp")
        .version(crate_version!())
        .arg(Arg::with_name("first").required(true))
        .arg(Arg::with_name("second").required(true))
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

    config::set_config(
        matches.value_of_os("first").unwrap().into(),
        matches.value_of_os("second").unwrap().into(),
        full_compare_limit,
        ignored_dirs,
    );

    let entries = (
        EntryInfo::new(config::get_config().first().clone())?,
        EntryInfo::new(config::get_config().second().clone())?,
    );

    let result = if let Some(content_size) = content_size {
        entries.0.contents_eq(entries.1, content_size)?
    } else {
        entries.0.entry_eq(entries.1)?
    };

    match result {
        Comparison::Equal => return Ok(true),
        _ => eprintln!("{:?}", result),
    }

    Ok(false)
}

fn main() {
    match run() {
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        _ => (),
    }
}
