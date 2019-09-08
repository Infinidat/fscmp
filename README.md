# fscmp [![Latest version](https://img.shields.io/crates/v/fscmp.svg)](https://crates.io/crates/fscmp)

Utility for comparing files/directories.

# Logging

By default `simplelog` is used for logging if `--log-dir` is passed. If default features are
disabled and `loggest` is enabled instead then that is used for logging.

Note: `simplelog` and `loggest` are mutually exclusive. If both features are enabled a run-time
panic will occur.
