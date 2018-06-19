mod diff;

use self::diff::Diff;
use super::config;
use super::file_ext_exact::FileExtExact;
use rayon::prelude::*;
use std;
use std::cmp::{max, min};
use std::collections::hash_map;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

const BLOCK_SIZE: usize = 512;

trait SliceRange {
    fn subslice(&self, start: usize, size: usize) -> &Self;
}

impl<T> SliceRange for [T] {
    fn subslice(&self, start: usize, size: usize) -> &Self {
        let end = min(start + size, self.len());
        return &self[start..end];
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Comparison {
    Equal,
    Unequal { diff: Diff, path: PathBuf },
}

impl Comparison {
    fn unequal(diff: Diff, first: &EntryInfo, second: &EntryInfo) -> Comparison {
        let path = first
            .path
            .strip_prefix(config::get_config().first())
            .unwrap()
            .into();

        assert_eq!(
            path,
            second
                .path
                .strip_prefix(config::get_config().second())
                .unwrap()
        );

        Comparison::Unequal { diff, path }
    }
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Comparison::Equal => Ok(()),
            Comparison::Unequal { diff, path } => {
                write!(f, "Mismatch in \"/{}\": {}", path.to_string_lossy(), diff)
            }
        }
    }
}

pub struct EntryInfo {
    path: PathBuf,
    metadata: fs::Metadata,
}

fn dir_entry_to_map(
    entry: Result<fs::DirEntry, io::Error>,
) -> Result<(PathBuf, fs::DirEntry), io::Error> {
    let entry = entry?;
    Ok((entry.file_name().into(), entry))
}

macro_rules! compare_metadata_field {
    ($first:ident, $second:ident, $accessor:ident, $err_type:path) => {
        if $first.metadata.$accessor() != $second.metadata.$accessor() {
            return Ok(Comparison::unequal(
                $err_type($first.metadata.$accessor(), $second.metadata.$accessor()),
                &$first,
                &$second,
            ));
        }
    };
}

fn entry_get<'a, K, V>(entry: &'a hash_map::Entry<K, V>) -> Option<&'a V> {
    match entry {
        hash_map::Entry::Vacant(_) => None,
        hash_map::Entry::Occupied(ref oe) => Some(oe.get()),
    }
}

impl EntryInfo {
    pub fn new(path: PathBuf) -> Result<EntryInfo, io::Error> {
        let metadata = path.symlink_metadata()?;
        Ok(EntryInfo { path, metadata })
    }

    fn child_entry(&self, name: &Path, metadata: fs::Metadata) -> EntryInfo {
        EntryInfo {
            path: self.path.join(name),
            metadata,
        }
    }

    pub fn entry_eq(self, other: Self) -> Result<Comparison, io::Error> {
        match *config::get_config().inode_maps().lock().unwrap() {
            [ref mut first_map, ref mut second_map] => {
                let entry = first_map.entry(self.metadata.ino());
                let other_entry = second_map.entry(other.metadata.ino());

                let is_new = {
                    let value = entry_get(&entry);
                    let other_value = entry_get(&other_entry);

                    if value != other_value {
                        return Ok(Comparison::unequal(
                            Diff::Inodes(value.cloned(), other_value.cloned()),
                            &self,
                            &other,
                        ));
                    }

                    value.is_none()
                };

                if is_new {
                    entry.or_insert(
                        self.path
                            .strip_prefix(config::get_config().first())
                            .unwrap()
                            .into(),
                    );
                    other_entry.or_insert(
                        other
                            .path
                            .strip_prefix(config::get_config().second())
                            .unwrap()
                            .into(),
                    );
                } else {
                    return Ok(Comparison::Equal);
                }
            }
        }

        compare_metadata_field!(self, other, mode, Diff::Modes);
        compare_metadata_field!(self, other, nlink, Diff::Nlinks);
        compare_metadata_field!(self, other, uid, Diff::Uids);
        compare_metadata_field!(self, other, gid, Diff::Gids);

        let file_type = self.metadata.file_type();
        if file_type.is_dir() {
            self.dir_eq(other)
        } else if file_type.is_file() {
            self.file_eq(other)
        } else if file_type.is_symlink() {
            self.symlink_eq(other)
        } else if file_type.is_block_device() {
            self.block_device_eq(other)
        } else if file_type.is_char_device() {
            self.char_device_eq(other)
        } else if file_type.is_fifo() {
            self.fifo_eq(other)
        } else if file_type.is_socket() {
            self.socket_eq(other)
        } else {
            panic!("Cannot compare, unknown type {:?}", file_type);
        }
    }

    fn dir_eq(self, other: Self) -> Result<Comparison, io::Error> {
        let contents: HashMap<_, _> = fs::read_dir(&self.path)?
            .map(dir_entry_to_map)
            .collect::<Result<_, _>>()?;
        let other_contents: HashMap<_, _> = fs::read_dir(&other.path)?
            .map(dir_entry_to_map)
            .collect::<Result<_, _>>()?;

        if contents.len() != other_contents.len() {
            return Ok(Comparison::unequal(
                Diff::DirContents(
                    contents.keys().cloned().collect(),
                    other_contents.keys().cloned().collect(),
                ),
                &self,
                &other,
            ));
        }

        contents
            .par_iter()
            .filter(|(name, _)| !config::get_config().ignored_dirs().contains::<Path>(name))
            .map(|(name, entry)| {
                if let Some(other_entry) = other_contents.get::<Path>(name) {
                    let first = self.child_entry(&name, entry.metadata()?);
                    let second = other.child_entry(&name, other_entry.metadata()?);
                    first.entry_eq(second)
                } else {
                    Ok(Comparison::unequal(
                        Diff::DirContents(
                            contents.keys().cloned().collect(),
                            other_contents.keys().cloned().collect(),
                        ),
                        &self,
                        &other,
                    ))
                }
            })
            .find_any(|r| r.as_ref().ok() != Some(&Comparison::Equal))
            .unwrap_or(Ok(Comparison::Equal))
    }

    fn file_eq(self, other: Self) -> Result<Comparison, io::Error> {
        compare_metadata_field!(self, other, len, Diff::Sizes);

        let metadata_len = self.metadata.len();
        return self.contents_eq(other, metadata_len);
    }

    pub fn contents_eq(self, other: Self, size: u64) -> Result<Comparison, io::Error> {
        const BUF_SIZE: usize = 2 * 1024 * 1024;
        const BUF_SIZE_U64: u64 = BUF_SIZE as u64;

        let file1 = fs::File::open(&self.path)?;
        let file2 = fs::File::open(&other.path)?;

        let limit = config::get_config()
            .full_compare_limit()
            .map(|limit| min(limit, size))
            .unwrap_or(size);
        let leap = calc_leap(size, limit, BUF_SIZE_U64);

        (0..calc_chunk_count(limit, BUF_SIZE_U64))
            .into_par_iter()
            .map(|i| ((i * leap)..min(size, i * leap + BUF_SIZE_U64)))
            .map(|chunk| {
                let mut data1: [u8; BUF_SIZE] = unsafe { std::mem::uninitialized() };
                let mut data2: [u8; BUF_SIZE] = unsafe { std::mem::uninitialized() };

                let mut chunked_data1 = &mut data1[..(chunk.end - chunk.start) as usize];
                let mut chunked_data2 = &mut data2[..(chunk.end - chunk.start) as usize];

                file1.read_at_exact(&mut chunked_data1, chunk.start)?;
                file2.read_at_exact(&mut chunked_data2, chunk.start)?;

                Ok(if chunked_data1 == chunked_data2 {
                    Comparison::Equal
                } else {
                    let local_lba =
                        get_diff_index(chunked_data1, chunked_data2) / BLOCK_SIZE * BLOCK_SIZE;
                    let lba = (chunk.start as usize) + local_lba;
                    Comparison::unequal(
                        Diff::Contents(
                            lba as u64,
                            chunked_data1.subslice(local_lba, BLOCK_SIZE).to_vec(),
                            chunked_data2.subslice(local_lba, BLOCK_SIZE).to_vec(),
                        ),
                        &self,
                        &other,
                    )
                })
            })
            .find_any(|r| r.as_ref().ok() != Some(&Comparison::Equal))
            .unwrap_or(Ok(Comparison::Equal))
    }

    fn symlink_eq(self, other: Self) -> Result<Comparison, io::Error> {
        let self_target = fs::read_link(&self.path)?;
        let other_target = fs::read_link(&other.path)?;
        if self_target != other_target {
            return Ok(Comparison::unequal(
                Diff::Links(self_target, other_target),
                &self,
                &other,
            ));
        }

        Ok(Comparison::Equal)
    }

    fn block_device_eq(self, other: Self) -> Result<Comparison, io::Error> {
        return self.char_device_eq(other);
    }

    fn char_device_eq(self, other: Self) -> Result<Comparison, io::Error> {
        compare_metadata_field!(self, other, rdev, Diff::DeviceTypes);

        Ok(Comparison::Equal)
    }

    fn fifo_eq(self, _other: Self) -> Result<Comparison, io::Error> {
        Ok(Comparison::Equal)
    }

    fn socket_eq(self, _other: Self) -> Result<Comparison, io::Error> {
        Ok(Comparison::Equal)
    }
}

fn get_diff_index(first: &[u8], second: &[u8]) -> usize {
    for (i, (x, y)) in first.iter().zip(second.iter()).enumerate() {
        if x != y {
            return i;
        }
    }
    panic!();
}

fn calc_chunk_count(limit: u64, chunk_size: u64) -> u64 {
    max(limit / chunk_size, 1)
}

fn calc_leap(size: u64, limit: u64, chunk_size: u64) -> u64 {
    if limit < chunk_size {
        limit
    } else {
        max::<u64>(chunk_size, size / (limit / chunk_size))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_calc_leap() {
        assert_eq!(calc_leap(100, 50, 2), 4);
        assert_eq!(calc_leap(50, 50, 2), 2);
        assert_eq!(calc_leap(150, 30, 2), 10);
        assert_eq!(calc_leap(25, 50, 2), 2);
        assert_eq!(calc_leap(25, 1, 2), 1);
    }

    #[test]
    fn test_calc_chunk_count() {
        assert_eq!(calc_chunk_count(1, 2), 1);
        assert_eq!(calc_chunk_count(50, 2), 25);
        assert_eq!(calc_chunk_count(20, 2), 10);
    }
}
