// Copyright 2020 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{HashSet, VecDeque};
use std::fmt::{Debug, Error, Formatter};
use std::ops::Range;

use itertools::Itertools;

use crate::diff;
use crate::diff::{Diff, DiffHunk};

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DiffLine<'a> {
    pub left_line_number: u32,
    pub right_line_number: u32,
    pub has_left_content: bool,
    pub has_right_content: bool,
    pub hunks: Vec<DiffHunk<'a>>,
}

impl DiffLine<'_> {
    fn reset_line(&mut self) {
        self.has_left_content = false;
        self.has_right_content = false;
        self.hunks.clear();
    }

    pub fn is_unmodified(&self) -> bool {
        self.hunks
            .iter()
            .all(|hunk| matches!(hunk, DiffHunk::Matching(_)))
    }
}

pub fn diff<'a>(left: &'a [u8], right: &'a [u8]) -> DiffLineIterator<'a> {
    let diff_hunks = diff::diff(left, right);
    DiffLineIterator::new(diff_hunks)
}

pub struct DiffLineIterator<'a> {
    diff_hunks: Vec<DiffHunk<'a>>,
    current_pos: usize,
    current_line: DiffLine<'a>,
    queued_lines: VecDeque<DiffLine<'a>>,
}

impl<'a> DiffLineIterator<'a> {
    fn new(diff_hunks: Vec<DiffHunk<'a>>) -> Self {
        let current_line = DiffLine {
            left_line_number: 1,
            right_line_number: 1,
            has_left_content: false,
            has_right_content: false,
            hunks: vec![],
        };
        DiffLineIterator {
            diff_hunks,
            current_pos: 0,
            current_line,
            queued_lines: VecDeque::new(),
        }
    }
}

impl<'a> Iterator for DiffLineIterator<'a> {
    type Item = DiffLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: Should we attempt to interpret as utf-8 and otherwise break only at
        // newlines?
        while self.current_pos < self.diff_hunks.len() && self.queued_lines.is_empty() {
            let hunk = &self.diff_hunks[self.current_pos];
            self.current_pos += 1;
            match hunk {
                diff::DiffHunk::Matching(text) => {
                    let lines = text.split_inclusive(|b| *b == b'\n');
                    for line in lines {
                        self.current_line.has_left_content = true;
                        self.current_line.has_right_content = true;
                        self.current_line.hunks.push(DiffHunk::Matching(line));
                        if line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.left_line_number += 1;
                            self.current_line.right_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                }
                diff::DiffHunk::Different(contents) => {
                    let left_lines = contents[0].split_inclusive(|b| *b == b'\n');
                    for left_line in left_lines {
                        self.current_line.has_left_content = true;
                        self.current_line
                            .hunks
                            .push(DiffHunk::Different(vec![left_line, b""]));
                        if left_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.left_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                    let right_lines = contents[1].split_inclusive(|b| *b == b'\n');
                    for right_line in right_lines {
                        self.current_line.has_right_content = true;
                        self.current_line
                            .hunks
                            .push(DiffHunk::Different(vec![b"", right_line]));
                        if right_line.ends_with(b"\n") {
                            self.queued_lines.push_back(self.current_line.clone());
                            self.current_line.right_line_number += 1;
                            self.current_line.reset_line();
                        }
                    }
                }
            }
        }

        if let Some(line) = self.queued_lines.pop_front() {
            return Some(line);
        }

        if !self.current_line.hunks.is_empty() {
            let line = self.current_line.clone();
            self.current_line.reset_line();
            return Some(line);
        }

        None
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum MergeHunk {
    Resolved(Vec<u8>),
    Conflict {
        removes: Vec<Vec<u8>>,
        adds: Vec<Vec<u8>>,
    },
}

impl Debug for MergeHunk {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            MergeHunk::Resolved(data) => f
                .debug_tuple("Resolved")
                .field(&String::from_utf8_lossy(data))
                .finish(),
            MergeHunk::Conflict { removes, adds } => f
                .debug_struct("Conflict")
                .field(
                    "removes",
                    &removes
                        .iter()
                        .map(|part| String::from_utf8_lossy(part))
                        .collect_vec(),
                )
                .field(
                    "adds",
                    &adds
                        .iter()
                        .map(|part| String::from_utf8_lossy(part))
                        .collect_vec(),
                )
                .finish(),
        }
    }
}

#[derive(PartialEq, Eq, Clone)]
pub enum MergeResult {
    Resolved(Vec<u8>),
    Conflict(Vec<MergeHunk>),
}

impl Debug for MergeResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            MergeResult::Resolved(data) => f
                .debug_tuple("Resolved")
                .field(&String::from_utf8_lossy(data))
                .finish(),
            MergeResult::Conflict(hunks) => f.debug_tuple("Conflict").field(hunks).finish(),
        }
    }
}

/// A region where the base and two sides match.
#[derive(Debug, PartialEq, Eq, Clone)]
struct SyncRegion {
    base: Range<usize>,
    left: Range<usize>,
    right: Range<usize>,
}

// TODO: Should we require `add.len() == removes.len() + 1`? If that condition
// is false, it effectively means that we should pretend that there are empty
// strings in `removes` or `adds` to make it true. Maybe we should have to
// caller make it explicitly that way.
pub fn merge(removes: &[&[u8]], adds: &[&[u8]]) -> MergeResult {
    let num_removes = removes.len();
    // TODO: Using the first remove as base (first in the inputs) is how it's
    // usually done for 3-way conflicts. Are there better heuristics when there are
    // more than 3 parts?
    let mut diff_inputs = removes.to_vec();
    diff_inputs.extend(adds);

    let diff = Diff::for_tokenizer(&diff_inputs, &diff::find_line_ranges);
    let mut resolved_hunk: Vec<u8> = vec![];
    let mut merge_hunks: Vec<MergeHunk> = vec![];
    for diff_hunk in diff.hunks() {
        match diff_hunk {
            DiffHunk::Matching(content) => {
                if adds.len() > removes.len() {
                    resolved_hunk.extend(content);
                }
            }
            DiffHunk::Different(parts) => {
                let mut removed_parts = parts[..num_removes].to_vec();
                let mut added_parts = parts[num_removes..].to_vec();
                // Remove pairs of parts that match in the removes and adds.
                let mut added_index = 0;
                while added_index < added_parts.len() {
                    let added_part = added_parts[added_index];
                    added_index += 1;
                    for (removed_index, removed_part) in removed_parts.iter().enumerate() {
                        if *removed_part == added_part {
                            added_index -= 1;
                            added_parts.remove(added_index);
                            removed_parts.remove(removed_index);
                            break;
                        }
                    }
                }
                let distinct_removes: HashSet<&[u8]> = removed_parts.iter().copied().collect();
                let distinct_adds: HashSet<&[u8]> = added_parts.iter().copied().collect();
                if removed_parts.is_empty() && added_parts.is_empty() {
                    // The same content was added and removed, so there's
                    // nothing left.
                } else if distinct_removes.is_empty() && distinct_adds.len() == 1 {
                    // All sides added the same content
                    resolved_hunk.extend(added_parts[0]);
                } else if distinct_removes.len() == 1 && distinct_adds.is_empty() {
                    // All sides removed the same content
                } else if distinct_removes.len() == 1
                    && distinct_adds.len() == 1
                    && added_parts.len() == removed_parts.len() + 1
                {
                    // All sides made the same change, and there's a matching extra base to apply it
                    // to
                    resolved_hunk.extend(added_parts[0]);
                } else {
                    if !resolved_hunk.is_empty() {
                        merge_hunks.push(MergeHunk::Resolved(resolved_hunk));
                        resolved_hunk = vec![];
                    }
                    // Include the unfiltered lists of removed and added here, so the caller
                    // knows which part corresponds to which input.
                    merge_hunks.push(MergeHunk::Conflict {
                        removes: parts[..num_removes]
                            .iter()
                            .map(|part| part.to_vec())
                            .collect_vec(),
                        adds: parts[num_removes..]
                            .iter()
                            .map(|part| part.to_vec())
                            .collect_vec(),
                    });
                }
            }
        }
    }

    if merge_hunks.is_empty() {
        MergeResult::Resolved(resolved_hunk)
    } else {
        if !resolved_hunk.is_empty() {
            merge_hunks.push(MergeHunk::Resolved(resolved_hunk));
        }
        MergeResult::Conflict(merge_hunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge() {
        // Unchanged and empty on all sides
        assert_eq!(
            merge(&[b""], &[b"", b""]),
            MergeResult::Resolved(b"".to_vec())
        );
        // Unchanged on all sides
        assert_eq!(
            merge(&[b"a"], &[b"a", b"a"]),
            MergeResult::Resolved(b"a".to_vec())
        );
        // One side removed, one side unchanged
        assert_eq!(
            merge(&[b"a\n"], &[b"", b"a\n"]),
            MergeResult::Resolved(b"".to_vec())
        );
        // One side unchanged, one side removed
        assert_eq!(
            merge(&[b"a\n"], &[b"a\n", b""]),
            MergeResult::Resolved(b"".to_vec())
        );
        // Both sides removed same line
        assert_eq!(
            merge(&[b"a\n"], &[b"", b""]),
            MergeResult::Resolved(b"".to_vec())
        );
        // One side modified, one side unchanged
        assert_eq!(
            merge(&[b"a"], &[b"a b", b"a"]),
            MergeResult::Resolved(b"a b".to_vec())
        );
        // One side unchanged, one side modified
        assert_eq!(
            merge(&[b"a"], &[b"a", b"a b"]),
            MergeResult::Resolved(b"a b".to_vec())
        );
        // All sides added same content
        assert_eq!(
            merge(&[], &[b"a\n", b"a\n", b"a\n"]),
            MergeResult::Resolved(b"a\n".to_vec())
        );
        // One side modified, two sides added
        assert_eq!(
            merge(&[b"a"], &[b"b", b"b", b"b"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a".to_vec()],
                adds: vec![b"b".to_vec(), b"b".to_vec(), b"b".to_vec()]
            }])
        );
        // All sides removed same content
        assert_eq!(
            merge(&[b"a\n", b"a\n", b"a\n"], &[]),
            MergeResult::Resolved(b"".to_vec())
        );
        // One side modified, two sides removed
        assert_eq!(
            merge(&[b"a\n", b"a\n", b"a\n"], &[b""]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a\n".to_vec(), b"a\n".to_vec(), b"a\n".to_vec()],
                adds: vec![b"".to_vec()]
            }])
        );
        // Three sides made the same change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"b", b"b", b"b"]),
            MergeResult::Resolved(b"b".to_vec())
        );
        // One side unchanged, one side added
        assert_eq!(
            merge(&[b"a\n"], &[b"a\nb\n"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"".to_vec()],
                adds: vec![b"b\n".to_vec()]
            }])
        );
        // Two sides left one line unchanged, and added conflicting additional lines
        assert_eq!(
            merge(&[b"a\n"], &[b"a\nb\n", b"a\nc\n"]),
            MergeResult::Conflict(vec![
                MergeHunk::Resolved(b"a\n".to_vec()),
                MergeHunk::Conflict {
                    removes: vec![b"".to_vec()],
                    adds: vec![b"b\n".to_vec(), b"c\n".to_vec()]
                }
            ])
        );
        // One side removed, one side modified
        assert_eq!(
            merge(&[b"a\n"], &[b"", b"b\n"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a\n".to_vec()],
                adds: vec![b"".to_vec(), b"b\n".to_vec()]
            }])
        );
        // One side modified, one side removed
        assert_eq!(
            merge(&[b"a\n"], &[b"b\n", b""]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a\n".to_vec()],
                adds: vec![b"b\n".to_vec(), b"".to_vec()]
            }])
        );
        // Two sides modified in different ways
        assert_eq!(
            merge(&[b"a"], &[b"b", b"c"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a".to_vec()],
                adds: vec![b"b".to_vec(), b"c".to_vec()]
            }])
        );
        // Two of three sides don't change, third side changes
        assert_eq!(
            merge(&[b"a", b"a"], &[b"a", b"", b"a"]),
            MergeResult::Resolved(b"".to_vec())
        );
        // One side unchanged, two other sides make the same change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"", b"a", b""]),
            MergeResult::Resolved(b"".to_vec())
        );
        // One side unchanged, two other sides make the different change
        assert_eq!(
            merge(&[b"a", b"a"], &[b"b", b"a", b"c"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a".to_vec(), b"a".to_vec()],
                adds: vec![b"b".to_vec(), b"a".to_vec(), b"c".to_vec()]
            }])
        );
        // Merge of an unresolved conflict and another branch, where the other branch
        // undid the change from one of the inputs to the unresolved conflict in the
        // first.
        assert_eq!(
            merge(&[b"a", b"b"], &[b"b", b"a", b"c"]),
            MergeResult::Resolved(b"c".to_vec())
        );
        // Merge of an unresolved conflict and another branch.
        assert_eq!(
            merge(&[b"a", b"b"], &[b"c", b"d", b"e"]),
            MergeResult::Conflict(vec![MergeHunk::Conflict {
                removes: vec![b"a".to_vec(), b"b".to_vec()],
                adds: vec![b"c".to_vec(), b"d".to_vec(), b"e".to_vec()]
            }])
        );
    }
}
