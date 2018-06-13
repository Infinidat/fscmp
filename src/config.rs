use std::collections::HashSet;
use std::ffi::OsString;

#[derive(Debug, Getters)]
pub struct Config {
    #[get = "pub"]
    full_compare_limit: Option<u64>,
    #[get = "pub"]
    ignored_dirs: HashSet<OsString>,
}

static mut CONFIG: Option<Config> = None;

pub fn set_config(full_compare_limit: Option<u64>, ignored_dirs: HashSet<OsString>) {
    unsafe {
        if CONFIG.is_some() {
            panic!("Config is already set");
        }
        CONFIG = Some(Config {
            full_compare_limit,
            ignored_dirs,
        });
    }
}

pub fn get_config() -> &'static Config {
    unsafe { CONFIG.as_ref() }.unwrap()
}
