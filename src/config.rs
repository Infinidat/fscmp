use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Getters, Default)]
pub struct Config {
    #[get = "pub"]
    first: PathBuf,
    #[get = "pub"]
    second: PathBuf,
    #[get = "pub"]
    full_compare_limit: Option<u64>,
    #[get = "pub"]
    ignored_dirs: HashSet<PathBuf>,
    #[get = "pub"]
    inode_maps: Mutex<[HashMap<u64, PathBuf>; 2]>,
}

static mut CONFIG: Option<Config> = None;

pub fn set_config(
    first: PathBuf,
    second: PathBuf,
    full_compare_limit: Option<u64>,
    ignored_dirs: HashSet<PathBuf>,
) {
    unsafe {
        if CONFIG.is_some() {
            panic!("Config is already set");
        }
        CONFIG = Some(Config {
            first,
            second,
            full_compare_limit,
            ignored_dirs,
            ..Default::default()
        });
    }
}

pub fn get_config() -> &'static Config {
    unsafe { CONFIG.as_ref() }.unwrap()
}
