use std::cmp::min;
use std::ops::{Add, AddAssign, Range};

#[derive(Debug)]
pub struct RangeChunks<Idx> {
    start: Idx,
    size: Idx,
    leap: Idx,
    end: Option<Idx>,
}

pub trait ChunkableRange<Idx>
where
    Idx: Copy,
{
    fn chunks(&self, size: Idx) -> RangeChunks<Idx> {
        self.chunks_leap(size, size)
    }

    fn chunks_leap(&self, size: Idx, leap: Idx) -> RangeChunks<Idx>;
}

impl<Idx> ChunkableRange<Idx> for Range<Idx>
where
    Idx: Copy + Default,
{
    fn chunks_leap(&self, size: Idx, leap: Idx) -> RangeChunks<Idx> {
        RangeChunks {
            start: self.start,
            size,
            leap,
            end: Some(self.end),
        }
    }
}

impl<Idx> Iterator for RangeChunks<Idx>
where
    Idx: Ord + Add<Output = Idx> + AddAssign + Copy,
{
    type Item = Range<Idx>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut next_end = self.start + self.size;
        if let Some(end) = self.end {
            if self.start >= end {
                return None;
            } else {
                next_end = min::<Idx>(end, next_end);
            }
        }

        let result = Self::Item {
            start: self.start,
            end: next_end,
        };

        self.start += self.leap;
        Some(result)
    }
}

#[cfg(test)]
mod test {
    use super::ChunkableRange;

    #[test]
    fn test_range_chunks() {
        let range = 0..30;
        let mut chunks = range.chunks(8);

        assert_eq!(chunks.next().unwrap(), 0..8);
        assert_eq!(chunks.next().unwrap(), 8..16);
        assert_eq!(chunks.next().unwrap(), 16..24);
        assert_eq!(chunks.next().unwrap(), 24..30);
        assert_eq!(chunks.next(), None);
    }

    #[test]
    fn test_range_chunks_leap() {
        let range = 0..30;
        let mut chunks = range.chunks_leap(8, 12);

        assert_eq!(chunks.next().unwrap(), 0..8);
        assert_eq!(chunks.next().unwrap(), 12..20);
        assert_eq!(chunks.next().unwrap(), 24..30);
        assert_eq!(chunks.next(), None);
    }
}
