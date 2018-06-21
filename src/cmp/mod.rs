mod comparison;

pub use self::comparison::{Comparison, Diff};
use super::file_ext_exact::FileExtExact;
use rayon::prelude::*;
use std;
use std::cmp::{max, min};
use std::collections::hash_map;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

struct EntryInfo {
    path: PathBuf,
    metadata: fs::Metadata,
}

#[derive(Default)]
pub struct FSCmp {
    first: PathBuf,
    second: PathBuf,
    full_compare_limit: Option<u64>,
    ignored_dirs: HashSet<PathBuf>,
    inode_maps: Mutex<[HashMap<u64, PathBuf>; 2]>,
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
}

macro_rules! compare_metadata_field {
    ($self:ident, $first:ident, $second:ident, $accessor:ident, $err_type:path) => {
        if $first.metadata.$accessor() != $second.metadata.$accessor() {
            return Ok($self.unequal(
                $err_type($first.metadata.$accessor(), $second.metadata.$accessor()),
                &$first,
                &$second,
            ));
        }
    };
}

impl FSCmp {
    pub fn new(
        first: PathBuf,
        second: PathBuf,
        full_compare_limit: Option<u64>,
        ignored_dirs: HashSet<PathBuf>,
    ) -> FSCmp {
        FSCmp {
            first,
            second,
            full_compare_limit,
            ignored_dirs,
            ..Default::default()
        }
    }

    pub fn dirs(&self) -> Result<Comparison, io::Error> {
        self.entry_eq(
            EntryInfo::new(self.first.clone())?,
            EntryInfo::new(self.second.clone())?,
        )
    }

    pub fn contents(&self, size: u64) -> Result<Comparison, io::Error> {
        self.contents_eq(
            EntryInfo::new(self.first.clone())?,
            EntryInfo::new(self.second.clone())?,
            size,
        )
    }

    fn unequal(&self, diff: Diff, first: &EntryInfo, second: &EntryInfo) -> Comparison {
        let path = first.path.strip_prefix(&self.first).unwrap().into();

        assert_eq!(path, second.path.strip_prefix(&self.second).unwrap());

        let comp = Comparison::Unequal {
            diff,
            first: self.first.clone(),
            second: self.second.clone(),
            path,
        };
        debug!("{}", comp);
        comp
    }

    fn entry_eq(&self, first: EntryInfo, second: EntryInfo) -> Result<Comparison, io::Error> {
        debug!("Comparing {:?} and {:?}", first.path, second.path);

        match *self.inode_maps.lock().unwrap() {
            [ref mut first_map, ref mut second_map] => {
                let first_entry = first_map.entry(first.metadata.ino());
                let second_entry = second_map.entry(second.metadata.ino());

                let is_new = {
                    let first_value = entry_get(&first_entry);
                    let second_value = entry_get(&second_entry);

                    if first_value != second_value {
                        return Ok(self.unequal(
                            Diff::Inodes(first_value.cloned(), second_value.cloned()),
                            &first,
                            &second,
                        ));
                    }

                    first_value.is_none()
                };

                if is_new {
                    first_entry.or_insert(first.path.strip_prefix(&self.first).unwrap().into());
                    second_entry.or_insert(second.path.strip_prefix(&self.second).unwrap().into());
                } else {
                    return Ok(Comparison::Equal);
                }
            }
        }

        compare_metadata_field!(self, first, second, mode, Diff::Modes);
        compare_metadata_field!(self, first, second, nlink, Diff::Nlinks);
        compare_metadata_field!(self, first, second, uid, Diff::Uids);
        compare_metadata_field!(self, first, second, gid, Diff::Gids);

        let file_type = first.metadata.file_type();
        if file_type.is_dir() {
            self.dir_eq(first, second)
        } else if file_type.is_file() {
            self.file_eq(first, second)
        } else if file_type.is_symlink() {
            self.symlink_eq(first, second)
        } else if file_type.is_block_device() {
            self.block_device_eq(first, second)
        } else if file_type.is_char_device() {
            self.char_device_eq(first, second)
        } else if file_type.is_fifo() {
            self.fifo_eq(first, second)
        } else if file_type.is_socket() {
            self.socket_eq(first, second)
        } else {
            panic!("Cannot compare, unknown type {:?}", file_type);
        }
    }

    fn dir_eq(&self, first: EntryInfo, second: EntryInfo) -> Result<Comparison, io::Error> {
        let first_contents: HashMap<_, _> = fs::read_dir(&first.path)?
            .map(dir_entry_to_map)
            .collect::<Result<_, _>>()?;
        let second_contents: HashMap<_, _> = fs::read_dir(&second.path)?
            .map(dir_entry_to_map)
            .collect::<Result<_, _>>()?;

        if first_contents.len() != second_contents.len() {
            return Ok(self.unequal(
                Diff::DirContents(
                    first_contents.keys().cloned().collect(),
                    second_contents.keys().cloned().collect(),
                ),
                &first,
                &second,
            ));
        }

        first_contents
            .par_iter()
            .filter(|(name, _)| !self.ignored_dirs.contains::<Path>(name))
            .map(|(name, entry)| {
                if let Some(second_entry) = second_contents.get::<Path>(name) {
                    let first = first.child_entry(&name, entry.metadata()?);
                    let second = second.child_entry(&name, second_entry.metadata()?);
                    self.entry_eq(first, second)
                } else {
                    Ok(self.unequal(
                        Diff::DirContents(
                            first_contents.keys().cloned().collect(),
                            second_contents.keys().cloned().collect(),
                        ),
                        &first,
                        &second,
                    ))
                }
            })
            .find_any(|r| r.as_ref().ok() != Some(&Comparison::Equal))
            .unwrap_or(Ok(Comparison::Equal))
    }

    fn file_eq(&self, first: EntryInfo, second: EntryInfo) -> Result<Comparison, io::Error> {
        compare_metadata_field!(self, first, second, len, Diff::Sizes);

        let metadata_len = first.metadata.len();
        return self.contents_eq(first, second, metadata_len);
    }

    fn contents_eq(
        &self,
        first: EntryInfo,
        second: EntryInfo,
        size: u64,
    ) -> Result<Comparison, io::Error> {
        const BUF_SIZE: usize = 2 * 1024 * 1024;
        const BUF_SIZE_U64: u64 = BUF_SIZE as u64;

        debug!(
            "Comparing contents of {:?} and {:?} of size {}",
            first.path, second.path, size
        );

        let file1 = fs::File::open(&first.path)?;
        let file2 = fs::File::open(&second.path)?;

        let limit = self.full_compare_limit
            .map(|limit| min(limit, size))
            .unwrap_or(size);
        let leap = calc_leap(size, limit, BUF_SIZE_U64);

        (0..calc_chunk_count(limit, BUF_SIZE_U64))
            .into_par_iter()
            .map(|i| ((i * leap)..min(size, i * leap + BUF_SIZE_U64)))
            .map(|chunk| {
                debug!(
                    "Comparing range [{}:{}) of {:?} and {:?}",
                    chunk.start, chunk.end, first.path, second.path
                );

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
                    self.unequal(
                        Diff::Contents(
                            lba as u64,
                            chunked_data1.subslice(local_lba, BLOCK_SIZE).to_vec(),
                            chunked_data2.subslice(local_lba, BLOCK_SIZE).to_vec(),
                        ),
                        &first,
                        &second,
                    )
                })
            })
            .find_any(|r| r.as_ref().ok() != Some(&Comparison::Equal))
            .unwrap_or({
                debug!("Compare of {:?} and {:?} finished", first.path, second.path);
                Ok(Comparison::Equal)
            })
    }

    fn symlink_eq(&self, first: EntryInfo, second: EntryInfo) -> Result<Comparison, io::Error> {
        let first_target = fs::read_link(&first.path)?;
        let second_target = fs::read_link(&second.path)?;
        if first_target != second_target {
            return Ok(self.unequal(Diff::Links(first_target, second_target), &first, &second));
        }

        Ok(Comparison::Equal)
    }

    fn block_device_eq(
        &self,
        first: EntryInfo,
        second: EntryInfo,
    ) -> Result<Comparison, io::Error> {
        return self.char_device_eq(first, second);
    }

    fn char_device_eq(&self, first: EntryInfo, second: EntryInfo) -> Result<Comparison, io::Error> {
        compare_metadata_field!(self, first, second, rdev, Diff::DeviceTypes);

        Ok(Comparison::Equal)
    }

    fn fifo_eq(&self, _first: EntryInfo, _second: EntryInfo) -> Result<Comparison, io::Error> {
        Ok(Comparison::Equal)
    }

    fn socket_eq(&self, _first: EntryInfo, _second: EntryInfo) -> Result<Comparison, io::Error> {
        Ok(Comparison::Equal)
    }
}

fn entry_get<'a, K, V>(entry: &'a hash_map::Entry<K, V>) -> Option<&'a V> {
    match entry {
        hash_map::Entry::Vacant(_) => None,
        hash_map::Entry::Occupied(ref oe) => Some(oe.get()),
    }
}

fn dir_entry_to_map(
    entry: Result<fs::DirEntry, io::Error>,
) -> Result<(PathBuf, fs::DirEntry), io::Error> {
    let entry = entry?;
    Ok((entry.file_name().into(), entry))
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
