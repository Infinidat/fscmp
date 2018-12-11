use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Comparison {
    Equal,
    Unequal {
        diff: Diff,
        first: PathBuf,
        second: PathBuf,
        path: Option<PathBuf>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Diff {
    Modes(u32, u32),
    Nlinks(u64, u64),
    Uids(u32, u32),
    Gids(u32, u32),
    Inodes(Option<PathBuf>, Option<PathBuf>),
    Sizes(i64, i64),
    Contents(u64, Vec<u8>, Vec<u8>),
    DeviceTypes(u64, u64),
    LinkTarget(PathBuf, PathBuf),
    DirContents(HashSet<PathBuf>, HashSet<PathBuf>),
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Comparison::Equal => Ok(()),
            Comparison::Unequal {
                diff,
                first: first_path,
                second: second_path,
                path,
            } => {
                let first_path = first_path.to_string_lossy();
                let second_path = second_path.to_string_lossy();
                write!(f, "Mismatch")?;
                if let Some(path) = path {
                    write!(f, " in \"{}\"", path.to_string_lossy())?;
                }
                write!(f, ": ")?;
                match diff {
                    Diff::Modes(first, second) => write!(
                        f,
                        "File mode\nFrom \"{}\": 0o{:o}\nFrom \"{}\": 0o{:o}",
                        first_path, first, second_path, second
                    ),
                    Diff::Nlinks(first, second) => write!(
                        f,
                        "Hard links number\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path, first, second_path, second
                    ),
                    Diff::Uids(first, second) => write!(
                        f,
                        "UID\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path, first, second_path, second
                    ),
                    Diff::Gids(first, second) => write!(
                        f,
                        "GID\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path, first, second_path, second
                    ),
                    Diff::Inodes(first, second) => write!(
                        f,
                        "Inodes\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path,
                        OptionFormat(first),
                        second_path,
                        OptionFormat(second)
                    ),
                    Diff::Sizes(first, second) => write!(
                        f,
                        "Size\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path, first, second_path, second
                    ),
                    Diff::Contents(lba, first, second) => write!(
                        f,
                        "Block {}\nFrom \"{}\":\n{}\nFrom \"{}\":\n{}",
                        lba,
                        first_path,
                        BlockFormat(first),
                        second_path,
                        BlockFormat(second)
                    ),
                    Diff::DeviceTypes(first, second) => write!(
                        f,
                        "Device type\nFrom \"{}\": {}\nFrom \"{}\": {}",
                        first_path, first, second_path, second
                    ),
                    Diff::LinkTarget(first, second) => write!(
                        f,
                        "Link target\nFrom \"{}\": \"{}\"\nFrom \"{}\": \"{}\"",
                        first_path,
                        first.display(),
                        second_path,
                        second.display()
                    ),
                    Diff::DirContents(first, second) => write!(
                        f,
                        "Dir contents\nFrom \"{}\": {:#?}\nFrom \"{}\": {:#?}",
                        first_path, first, second_path, second
                    ),
                }
            }
        }
    }
}

struct BlockFormat<'a>(&'a [u8]);

impl<'a> fmt::Display for BlockFormat<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const BYTES_IN_LINE: usize = 32;

        for chunk in self.0.chunks(BYTES_IN_LINE) {
            for b in chunk {
                write!(f, "{:02x} ", b)?;
            }
            writeln!(f)?;
        }

        Ok(())
    }
}

struct OptionFormat<'a, T>(&'a Option<T>);

impl<'a> fmt::Display for OptionFormat<'a, PathBuf> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "-"),
            Some(x) => write!(f, "\"{}\"", x.display()),
        }
    }
}
