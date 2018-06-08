#[macro_use]
extern crate clap;
extern crate failure;
#[macro_use]
extern crate failure_derive;

use clap::{App, Arg};
use std::fs;
use std::io::prelude::*;
use std::os::unix::fs::FileTypeExt;

#[derive(Debug, Fail)]
enum Error {
    #[fail(display = "Types differ: {:?} != {:?}", _0, _1)]
    TypesDiffer(std::fs::FileType, std::fs::FileType),

    #[fail(display = "Sizes differ: {} != {}", _0, _1)]
    SizesDiffer(u64, u64),

    #[fail(display = "Contents differ at offset {}: {:?} != {:?}", _0, _1, _2)]
    ContentsDiffer(usize, Vec<u8>, Vec<u8>),

    #[fail(display = "Cannot compare, unknown type {:?}", _0)]
    UnknownType(std::fs::FileType),
}

struct EntryInfo<'a> {
    name: &'a str,
    metadata: fs::Metadata,
}

impl<'a> EntryInfo<'a> {
    fn new(name: &'a str) -> Result<EntryInfo, std::io::Error> {
        Ok(EntryInfo {
            name,
            metadata: fs::metadata(name)?,
        })
    }

    fn entry_eq(&self, other: &Self) -> Result<(), failure::Error> {
        let file_type = self.metadata.file_type();
        {
            let other_file_type = other.metadata.file_type();
            if file_type != other_file_type {
                Err(Error::TypesDiffer(file_type, other_file_type))?;
            }
        }
        if file_type.is_dir() {
            return self.dir_eq(&other);
        } else if file_type.is_file() {
            return self.file_eq(&other);
        } else if file_type.is_symlink() {
            return self.symlink_eq(&other);
        } else if file_type.is_block_device() {
            return self.block_device_eq(&other);
        } else if file_type.is_char_device() {
            return self.char_device_eq(&other);
        } else if file_type.is_fifo() {
            return self.fifo_eq(&other);
        } else if file_type.is_socket() {
            return self.socket_eq(&other);
        } else {
            Err(Error::UnknownType(file_type))?
        }

        Ok(())
    }

    fn dir_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }

    fn file_eq(&self, other: &Self) -> Result<(), failure::Error> {
        if self.metadata.len() != other.metadata.len() {
            Err(Error::SizesDiffer(
                self.metadata.len(),
                other.metadata.len(),
            ))?
        }

        let mut file1 = fs::File::open(self.name)?;
        let mut file2 = fs::File::open(other.name)?;
        let mut data1 = vec![0u8; 2 * 1024 * 1024];
        let mut data2 = vec![0u8; 2 * 1024 * 1024];
        let mut remaining = self.metadata.len() as usize;
        while remaining > 0 {
            if remaining < data1.len() {
                data1.resize(remaining, 0);
                data2.resize(remaining, 0);
            }
            file1.read_exact(&mut data1)?;
            file2.read_exact(&mut data2)?;

            if data1 != data2 {
                return Err(Error::ContentsDiffer(
                    (self.metadata.len() as usize) - remaining,
                    data1,
                    data2,
                ))?;
            }

            remaining -= data1.len();
        }

        Ok(())
    }

    fn symlink_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }

    fn block_device_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }

    fn char_device_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }

    fn fifo_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }

    fn socket_eq(&self, other: &Self) -> Result<(), failure::Error> {
        Ok(())
    }
}

fn run() -> Result<(), failure::Error> {
    let matches = App::new("fscmp")
        .version(crate_version!())
        .arg(Arg::with_name("first").required(true))
        .arg(Arg::with_name("second").required(true))
        .get_matches();

    let entries = (
        EntryInfo::new(matches.value_of("first").unwrap())?,
        EntryInfo::new(matches.value_of("second").unwrap())?,
    );
    return entries.0.entry_eq(&entries.1);
}

fn main() {
    match run() {
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        _ => (),
    }
}
