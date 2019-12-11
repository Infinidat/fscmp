mod comparison;

pub use self::comparison::{Comparison, Diff};
use failure::{Fallible, ResultExt};
use libc;
use log::debug;
use nix::fcntl;
use nix::sys::stat::Mode;
use openat::{self, Dir};
use rayon::prelude::*;
use std::cmp::{max, min};
use std::collections::hash_map;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const BLOCK_SIZE: usize = 512;
const BUF_SIZE: usize = 256 * 1024;
const BUF_SIZE_U64: u64 = BUF_SIZE as u64;

#[repr(align(512))]
struct AlignedBuffer([u8; BUF_SIZE]);

trait SliceRange {
    fn subslice(&self, start: usize, size: usize) -> &Self;
}

impl<T> SliceRange for [T] {
    fn subslice(&self, start: usize, size: usize) -> &Self {
        let end = min(start + size, self.len());
        &self[start..end]
    }
}

struct EntryInfo {
    parent: Arc<Dir>,
    parent_path: PathBuf,
    path: PathBuf,
    metadata: openat::Metadata,
}

#[derive(Default)]
pub struct FSCmp {
    first: PathBuf,
    second: PathBuf,
    full_compare_limit: Option<u64>,
    ignored_dirs: HashSet<PathBuf>,
    inode_maps: Mutex<[HashMap<libc::ino_t, PathBuf>; 2]>,
}

impl EntryInfo {
    fn dir(path: &Path) -> Fallible<EntryInfo> {
        assert!(path.is_dir());
        let path = path.canonicalize()?;
        let dir = Dir::open(&path)?;
        let path = ".".to_string().into();
        let metadata = dir.metadata(&path)?;
        Ok(EntryInfo {
            parent: Arc::new(dir),
            parent_path: Default::default(),
            path,
            metadata,
        })
    }

    fn file(path: &Path) -> Fallible<EntryInfo> {
        assert!(!path.is_dir());
        let path = path.canonicalize()?;
        let dir = Dir::open(path.parent().unwrap())?;
        let path = path.file_name().unwrap().to_os_string().into();
        let metadata = dir.metadata(&path)?;
        Ok(EntryInfo {
            parent: Arc::new(dir),
            parent_path: Default::default(),
            path,
            metadata,
        })
    }

    fn child_entry(&self, name: &Path) -> Fallible<EntryInfo> {
        let path = if self.path.starts_with(".") {
            name.to_path_buf()
        } else {
            self.path.join(name)
        };

        Ok(if path.as_os_str().len() > libc::PATH_MAX as usize {
            let path_parent = path.parent().unwrap();
            let parent_path = self.parent_path.join(path_parent);
            let dir = self.parent.sub_dir(path_parent)?;
            let path = path.file_name().unwrap().to_os_string().into();
            let metadata = dir.metadata(&path)?;
            EntryInfo {
                parent: Arc::new(dir),
                parent_path,
                path,
                metadata,
            }
        } else {
            let dir = self.parent.clone();
            let metadata = dir.metadata(&path)?;
            EntryInfo {
                parent: dir,
                parent_path: self.parent_path.clone(),
                path,
                metadata,
            }
        })
    }
}

macro_rules! compare_metadata_field {
    ($self:ident, $first:ident, $second:ident, $field:ident, $err_type:path) => {
        if $first.metadata.stat().$field != $second.metadata.stat().$field {
            return Ok($self.unequal(
                $err_type($first.metadata.stat().$field, $second.metadata.stat().$field),
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
    ) -> Self {
        Self {
            first,
            second,
            full_compare_limit,
            ignored_dirs,
            ..Default::default()
        }
    }

    pub fn dirs(&self) -> Fallible<Comparison> {
        self.entry_eq(&EntryInfo::dir(&self.first)?, &EntryInfo::dir(&self.second)?)
    }

    pub fn contents(&self, size: u64) -> Fallible<Comparison> {
        self.contents_eq(&EntryInfo::file(&self.first)?, &EntryInfo::file(&self.second)?, size)
    }

    fn unequal(&self, diff: Diff, first: &EntryInfo, second: &EntryInfo) -> Comparison {
        let comp = Comparison::Unequal {
            diff,
            first: self.first.clone(),
            second: self.second.clone(),
            path: if first.path == second.path {
                Some(first.parent_path.join(&first.path))
            } else {
                None
            },
        };
        debug!("{}", comp);
        comp
    }

    fn entry_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        debug!(
            "Comparing \"{}\" and \"{}\"",
            first.path.display(),
            second.path.display()
        );

        match *self.inode_maps.lock().unwrap() {
            [ref mut first_map, ref mut second_map] => {
                let first_entry = first_map.entry(first.metadata.stat().st_ino);
                let second_entry = second_map.entry(second.metadata.stat().st_ino);

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
                    first_entry.or_insert_with(|| first.path.clone());
                    second_entry.or_insert_with(|| second.path.clone());
                } else {
                    return Ok(Comparison::Equal);
                }
            }
        }

        if first.path != Path::new(".") {
            compare_metadata_field!(self, first, second, st_mode, Diff::Modes);
            compare_metadata_field!(self, first, second, st_uid, Diff::Uids);
            compare_metadata_field!(self, first, second, st_gid, Diff::Gids);
        }
        compare_metadata_field!(self, first, second, st_nlink, Diff::Nlinks);

        let file_type = first.metadata.stat().st_mode & libc::S_IFMT;
        match file_type {
            libc::S_IFDIR => self.dir_eq(first, second),
            libc::S_IFREG => self.file_eq(first, second),
            libc::S_IFLNK => self.symlink_eq(first, second),
            libc::S_IFBLK => self.block_device_eq(first, second),
            libc::S_IFCHR => self.char_device_eq(first, second),
            libc::S_IFIFO => self.fifo_eq(first, second),
            libc::S_IFSOCK => self.socket_eq(first, second),
            _ => panic!("Cannot compare, unknown type {:#o}", file_type),
        }
    }

    fn entry_filter_map(&self, path_res: io::Result<openat::Entry>) -> Option<io::Result<PathBuf>> {
        match path_res {
            Ok(path) => {
                let path = Path::new(path.file_name());
                if self.ignored_dirs.contains::<Path>(path) {
                    None
                } else {
                    Some(Ok(PathBuf::from(path)))
                }
            }
            Err(e) => Some(Err(e)),
        }
    }

    fn list_dir(&self, entry: &EntryInfo) -> io::Result<HashSet<PathBuf>> {
        entry
            .parent
            .list_dir(&entry.path)?
            .filter_map(|p| self.entry_filter_map(p))
            .collect::<Result<_, _>>()
    }

    fn dir_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        let first_contents: HashSet<_> = self.list_dir(first).context("first")?;
        let second_contents: HashSet<_> = self.list_dir(second).context("second")?;

        if first_contents.len() != second_contents.len() {
            return Ok(self.unequal(Diff::DirContents(first_contents, second_contents), &first, &second));
        }

        first_contents
            .par_iter()
            .map(|name| {
                if second_contents.contains(name) {
                    let first = first.child_entry(&name)?;
                    let second = second.child_entry(&name)?;
                    self.entry_eq(&first, &second)
                } else {
                    Ok(self.unequal(
                        Diff::DirContents(first_contents.clone(), second_contents.clone()),
                        &first,
                        &second,
                    ))
                }
            })
            .find_any(|r| r.as_ref().ok() != Some(&Comparison::Equal))
            .unwrap_or(Ok(Comparison::Equal))
    }

    fn file_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        compare_metadata_field!(self, first, second, st_size, Diff::Sizes);

        let metadata_len = first.metadata.len();
        self.contents_eq(first, second, metadata_len)
    }

    fn contents_eq(&self, first: &EntryInfo, second: &EntryInfo, size: u64) -> Fallible<Comparison> {
        fn open_file(info: &EntryInfo) -> nix::Result<File> {
            unsafe {
                Ok(File::from_raw_fd(fcntl::openat(
                    info.parent.as_raw_fd(),
                    &info.path,
                    #[cfg(not(test))]
                    fcntl::OFlag::O_DIRECT,
                    #[cfg(test)]
                    fcntl::OFlag::empty(),
                    Mode::empty(),
                )?))
            }
        }

        if size == 0 {
            return Ok(Comparison::Equal);
        }

        debug!(
            "Comparing contents of \"{}\" and \"{}\" of size {}",
            first.path.display(),
            second.path.display(),
            size
        );

        let file1 = open_file(first)?;
        let file2 = open_file(second)?;

        let limit = self.full_compare_limit.map(|limit| min(limit, size)).unwrap_or(size);
        let leap = calc_leap(size, limit, BUF_SIZE_U64);

        (0..calc_chunk_count(limit, BUF_SIZE_U64))
            .into_par_iter()
            .map(|i| ((i * leap)..min(size, i * leap + BUF_SIZE_U64)))
            .map(|chunk| {
                debug!(
                    "Comparing range [{}:{}) of \"{}\" and \"{}\"",
                    chunk.start,
                    chunk.end,
                    first.path.display(),
                    second.path.display()
                );

                let mut buffer1 = AlignedBuffer(unsafe { std::mem::MaybeUninit::uninit().assume_init() });
                let mut buffer2 = AlignedBuffer(unsafe { std::mem::MaybeUninit::uninit().assume_init() });
                let data1 = &mut buffer1.0;
                let data2 = &mut buffer2.0;

                let mut chunked_data1 = &mut data1[..(chunk.end - chunk.start) as usize];
                let mut chunked_data2 = &mut data2[..(chunk.end - chunk.start) as usize];

                file1
                    .read_exact_at(&mut chunked_data1, chunk.start)
                    .with_context(|e| format!("\"{}\": {}", first.path.display().to_string(), e))?;
                file2
                    .read_exact_at(&mut chunked_data2, chunk.start)
                    .with_context(|e| format!("\"{}\": {}", second.path.display().to_string(), e))?;

                Ok(if chunked_data1 == chunked_data2 {
                    Comparison::Equal
                } else {
                    let diff_index = get_diff_index(chunked_data1, chunked_data2);
                    let local_lba = diff_index / BLOCK_SIZE * BLOCK_SIZE;
                    let lba = ((chunk.start as usize) + diff_index) / BLOCK_SIZE;
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
            .unwrap_or_else(|| {
                debug!(
                    "Compare of \"{}\" and \"{}\" finished",
                    first.path.display(),
                    second.path.display()
                );
                Ok(Comparison::Equal)
            })
    }

    fn symlink_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        let first_target = first.parent.read_link(&first.path)?;
        let second_target = second.parent.read_link(&second.path)?;
        if first_target != second_target {
            return Ok(self.unequal(Diff::LinkTarget(first_target, second_target), &first, &second));
        }

        Ok(Comparison::Equal)
    }

    fn block_device_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        self.char_device_eq(first, second)
    }

    fn char_device_eq(&self, first: &EntryInfo, second: &EntryInfo) -> Fallible<Comparison> {
        compare_metadata_field!(self, first, second, st_rdev, Diff::DeviceTypes);

        Ok(Comparison::Equal)
    }

    fn fifo_eq(&self, _first: &EntryInfo, _second: &EntryInfo) -> Fallible<Comparison> {
        Ok(Comparison::Equal)
    }

    fn socket_eq(&self, _first: &EntryInfo, _second: &EntryInfo) -> Fallible<Comparison> {
        Ok(Comparison::Equal)
    }
}

fn entry_get<'a, K, V>(entry: &'a hash_map::Entry<K, V>) -> Option<&'a V> {
    match entry {
        hash_map::Entry::Vacant(_) => None,
        hash_map::Entry::Occupied(ref oe) => Some(oe.get()),
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
        max::<u64>(chunk_size, size / ((limit + chunk_size - 1) / chunk_size))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;
    use std::io;
    use std::io::prelude::*;
    use std::os::unix;
    use std::os::unix::fs::PermissionsExt;
    use tempfile;
    use walkdir;

    #[test]
    fn test_calc_leap() {
        assert_eq!(calc_leap(100, 50, 2), 4);
        assert_eq!(calc_leap(50, 50, 2), 2);
        assert_eq!(calc_leap(150, 30, 2), 10);
        assert_eq!(calc_leap(25, 50, 2), 2);
        assert_eq!(calc_leap(25, 1, 2), 1);
        assert_eq!(calc_leap(2_000_000_000, 2_000_000_000, BUF_SIZE_U64), BUF_SIZE_U64);
    }

    #[test]
    fn test_calc_chunk_count() {
        assert_eq!(calc_chunk_count(1, 2), 1);
        assert_eq!(calc_chunk_count(50, 2), 25);
        assert_eq!(calc_chunk_count(20, 2), 10);
    }

    fn mknod(path: PathBuf, mode: libc::mode_t, dev: libc::dev_t) -> Fallible<()> {
        use std::ffi;
        use std::os::unix::ffi::OsStringExt;

        unsafe {
            libc::mknod(
                ffi::CString::new(path.into_os_string().into_vec())?.as_ptr(),
                mode | 0o644,
                dev,
            )
        };

        Ok(())
    }

    fn generate_tree() -> Fallible<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        for dir in &[dir.path(), &dir.path().join("directory")] {
            fs::create_dir(dir.join("directory"))?;
            File::create(dir.join("regular_file"))?;
            unix::fs::symlink("symlink_target", dir.join("symlink"))?;
            mknod(dir.join("block_device"), libc::S_IFBLK, 0)?;
            mknod(dir.join("char_device"), libc::S_IFCHR, 0)?;
            mknod(dir.join("fifo"), libc::S_IFIFO, 0)?;
            mknod(dir.join("socket"), libc::S_IFSOCK, 0)?;
        }
        Ok(dir)
    }

    #[test]
    fn test_simple() -> Fallible<()> {
        let dir1 = generate_tree()?;
        let fscmp = FSCmp::new(dir1.path().into(), dir1.path().into(), None, HashSet::new());
        assert_eq!(fscmp.dirs()?, Comparison::Equal);

        let dir2 = generate_tree()?;
        let fscmp = FSCmp::new(dir1.path().into(), dir2.path().into(), None, HashSet::new());
        assert_eq!(fscmp.dirs()?, Comparison::Equal);

        File::create(dir2.path().join("new_regular_file"))?;
        let fscmp = FSCmp::new(dir1.path().into(), dir2.path().into(), None, HashSet::new());
        if let Comparison::Unequal {
            diff: Diff::DirContents(..),
            ..
        } = fscmp.dirs()?
        {
        } else {
            panic!("New file not detected");
        }
        fs::remove_file(dir2.path().join("new_regular_file"))?;
        Ok(())
    }

    #[test]
    fn test_permissions() -> Fallible<()> {
        let dir1 = generate_tree()?;
        let dir2 = generate_tree()?;
        for entry in walkdir::WalkDir::new(dir1.path())
            .min_depth(1)
            .into_iter()
            .filter(|e| !e.as_ref().unwrap().path_is_symlink())
        {
            let entry = entry?;
            let original_perms = fs::symlink_metadata(entry.path())?.permissions();
            let mut new_perms = original_perms.clone();
            new_perms.set_readonly(true);
            fs::set_permissions(entry.path(), new_perms)?;

            let fscmp = FSCmp::new(dir1.path().into(), dir2.path().into(), None, HashSet::new());
            if let Comparison::Unequal {
                diff: Diff::Modes(..),
                path: Some(path),
                ..
            } = fscmp.dirs()?
            {
                assert!(entry.path().ends_with(path));
            } else {
                panic!("Comparison should be unequal");
            }

            fs::set_permissions(entry.path(), original_perms)?;
        }
        Ok(())
    }

    #[test]
    fn test_contents() -> Fallible<()> {
        let dir1 = generate_tree()?;
        let dir2 = generate_tree()?;

        let file1_path = dir1.path().join("regular_file");
        let file2_path = dir2.path().join("regular_file");

        let fscmp = FSCmp::new(file1_path.clone(), file2_path.clone(), None, HashSet::new());
        assert_eq!(fscmp.contents(0)?, Comparison::Equal);

        let mut file1 = fs::OpenOptions::new().write(true).open(&file1_path)?;
        let file2 = fs::OpenOptions::new().write(true).open(&file2_path)?;

        file1.set_len(1024 * 1024)?;
        file2.set_len(1024 * 1024)?;
        let fscmp = FSCmp::new(file1_path.clone(), file2_path.clone(), None, HashSet::new());
        assert_eq!(fscmp.contents(1024 * 1024)?, Comparison::Equal);

        let offset = file1.seek(io::SeekFrom::Start(532 * 1024 + 13))?;
        file1.write_all(b"a")?;
        let fscmp = FSCmp::new(file1_path.clone(), file2_path.clone(), None, HashSet::new());
        if let Comparison::Unequal {
            diff: Diff::Contents(lba, ..),
            ..
        } = fscmp.contents(1024 * 1024)?
        {
            assert_eq!(offset / 512, lba);
        } else {
            panic!("Content should be unequal");
        }
        Ok(())
    }

    #[test]
    fn test_path_max() -> Fallible<()> {
        let dir = tempfile::tempdir()?;
        let parent = openat::Dir::open(dir.path())?;
        let name = "a".repeat(255);
        let mut dir_path: PathBuf = name.to_string().into();
        while dir_path.as_os_str().len() < libc::PATH_MAX as usize {
            parent.create_dir(&dir_path, 0o755)?;
            dir_path.push(&name);
        }
        let parent = parent.sub_dir(dir_path.parent().unwrap())?;
        let filename = dir_path.file_name().unwrap();
        parent.create_dir("a", 0o755)?;
        parent.new_file(filename, 0o644)?.write_all(b"a")?;

        let fscmp = FSCmp::new(dir.path().into(), dir.path().into(), None, HashSet::new());
        assert_eq!(fscmp.dirs()?, Comparison::Equal);
        Ok(())
    }

    #[test]
    fn test_root_mode() -> failure::Fallible<()> {
        let dir1 = tempfile::tempdir()?;
        let dir2 = tempfile::tempdir()?;
        fs::set_permissions(dir2.path(), fs::Permissions::from_mode(0o777))?;
        let fscmp = FSCmp::new(dir1.path().into(), dir2.path().into(), None, HashSet::new());
        assert_eq!(fscmp.dirs()?, Comparison::Equal);
        Ok(())
    }
}
