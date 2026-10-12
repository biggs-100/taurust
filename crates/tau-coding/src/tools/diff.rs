//! Diff rendering for the `edit` tool.
//!
//! Ported from tau's `generate_diff_string` and `generate_unified_patch`, which
//! sit on Python's `difflib`. Rust has no standard equivalent, so both are
//! implemented here by hand with no new dependency.
//!
//! [`SequenceMatcher`] is a line-level port of `difflib.SequenceMatcher`: the
//! same longest-matching-block search, the same matching-block collapse, the
//! same opcode derivation and the same `get_grouped_opcodes` windowing. It also
//! reproduces the `autojunk` heuristic (for 200 or more lines on the new side,
//! a line repeated more than `n / 100 + 1` times is excluded from matching),
//! which is on by default in Python and changes output for such inputs.
//!
//! Two parts of `difflib` are deliberately not reproduced, because they only
//! affect the human-facing diff text and never the file content:
//!
//! - `ndiff` runs `_fancy_replace` over a replaced block: a character-level diff
//!   of each line pair, `"? "` hint lines, a `0.74` similarity cutoff, and the
//!   pairing of a line present on both sides as context. This port emits the
//!   plain `"  "`, `"- "` and `"+ "` lines for a replaced block, in that order.
//!   That is not only a difference in hint lines. `_fancy_replace` is also what
//!   keeps `SequenceMatcher`'s `autojunk` collapse out of the printed diff, so
//!   for an input whose replaced block holds repeated lines this port prints the
//!   collapsed opcodes, with materially more `-` and `+` lines than the
//!   reference. Measured on the two `autojunk` cases: the 199-line case agrees
//!   exactly (200 lines, one deletion, one insertion), while for the 250-line
//!   case CPython 3.12.10 prints 251 lines with one deletion and one insertion
//!   where this port prints 375 lines with 125 of each. `first_changed_line`
//!   agrees in both cases.
//! - `difflib.unified_diff` never emits a `\ No newline at end of file` marker,
//!   and tau hands it `splitlines(keepends=True)` lines, so a final line without
//!   a terminator is emitted as it arrived and the next patch line is glued onto
//!   it. That is reproduced rather than corrected: the reference's patch text is
//!   the contract, however it reads. Verified against CPython 3.12.10.

use std::collections::HashMap;

/// How one side of a change relates to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// The two ranges are identical.
    Equal,
    /// The `a` range is absent from `b`.
    Delete,
    /// The `b` range is absent from `a`.
    Insert,
    /// The `a` range became the `b` range.
    Replace,
}

/// One operation needed to turn `a[i1..i2]` into `b[j1..j2]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Opcode {
    tag: Tag,
    i1: usize,
    i2: usize,
    j1: usize,
    j2: usize,
}

/// Render an `ndiff`-style line diff and the first changed line.
///
/// The line number is 1-based within the *new* content, matching the reference.
/// It is `None` when nothing changed.
pub fn generate_diff_string(old: &str, new: &str) -> (String, Option<usize>) {
    let matcher =
        SequenceMatcher::new(python_splitlines(old, false), python_splitlines(new, false));

    let mut lines: Vec<String> = Vec::new();
    for opcode in matcher.opcodes() {
        match opcode.tag {
            Tag::Equal => push_each(&mut lines, "  ", &matcher.a, opcode.i1, opcode.i2),
            Tag::Delete => push_each(&mut lines, "- ", &matcher.a, opcode.i1, opcode.i2),
            Tag::Insert => push_each(&mut lines, "+ ", &matcher.b, opcode.j1, opcode.j2),
            Tag::Replace => {
                push_each(&mut lines, "- ", &matcher.a, opcode.i1, opcode.i2);
                push_each(&mut lines, "+ ", &matcher.b, opcode.j1, opcode.j2);
            }
        }
    }

    let mut first_changed_line: Option<usize> = None;
    let mut new_line_number = 0usize;
    for line in &lines {
        if line.starts_with("  ") {
            new_line_number += 1;
        } else if line.starts_with('+') {
            new_line_number += 1;
            if first_changed_line.is_none() {
                first_changed_line = Some(new_line_number);
            }
        } else if line.starts_with('-') && first_changed_line.is_none() {
            first_changed_line = Some(new_line_number + 1);
        }
    }
    (lines.join("\n"), first_changed_line)
}

/// Render a unified patch with the reference's `---`/`+++` headers.
pub fn generate_unified_patch(path: &str, old: &str, new: &str) -> String {
    let matcher = SequenceMatcher::new(python_splitlines(old, true), python_splitlines(new, true));

    let mut patch = String::new();
    let mut started = false;
    for group in matcher.grouped_opcodes(3) {
        if !started {
            started = true;
            patch.push_str("--- ");
            patch.push_str(path);
            patch.push('\n');
            patch.push_str("+++ ");
            patch.push_str(path);
            patch.push('\n');
        }
        let first = &group[0];
        let last = &group[group.len() - 1];
        patch.push_str("@@ -");
        patch.push_str(&format_range_unified(first.i1, last.i2));
        patch.push_str(" +");
        patch.push_str(&format_range_unified(first.j1, last.j2));
        patch.push_str(" @@\n");

        for opcode in &group {
            match opcode.tag {
                Tag::Equal => push_patch_lines(&mut patch, ' ', &matcher.a, opcode.i1, opcode.i2),
                Tag::Delete => push_patch_lines(&mut patch, '-', &matcher.a, opcode.i1, opcode.i2),
                Tag::Insert => push_patch_lines(&mut patch, '+', &matcher.b, opcode.j1, opcode.j2),
                Tag::Replace => {
                    push_patch_lines(&mut patch, '-', &matcher.a, opcode.i1, opcode.i2);
                    push_patch_lines(&mut patch, '+', &matcher.b, opcode.j1, opcode.j2);
                }
            }
        }
    }
    patch
}

/// Append `prefix + line` for every line in `lines[from..to]`.
fn push_each(lines: &mut Vec<String>, prefix: &str, source: &[&str], from: usize, to: usize) {
    for line in &source[from..to] {
        lines.push(format!("{prefix}{line}"));
    }
}

/// Append one patch body line, exactly as `difflib` writes it.
///
/// The reference's lines already carry their terminators, so nothing is added
/// and nothing is repaired.
fn push_patch_lines(patch: &mut String, sign: char, source: &[&str], from: usize, to: usize) {
    for line in &source[from..to] {
        patch.push(sign);
        patch.push_str(line);
    }
}

/// Format a hunk range the way `difflib._format_range_unified` does.
///
/// A one-line range is just its number, and an empty range begins at the line
/// before it and carries a zero length.
fn format_range_unified(start: usize, stop: usize) -> String {
    let beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    let beginning = if length == 0 {
        beginning - 1
    } else {
        beginning
    };
    format!("{beginning},{length}")
}

/// Split text the way Python's `str.splitlines` does.
fn python_splitlines(text: &str, keepends: bool) -> Vec<&str> {
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if !is_linebreak(character) {
            continue;
        }
        let mut end = index + character.len_utf8();
        if character == '\r' {
            if let Some(&(next_index, '\n')) = characters.peek() {
                end = next_index + 1;
                characters.next();
            }
        }
        lines.push(&text[start..if keepends { end } else { index }]);
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Whether a character ends a line for Python's line splitting.
fn is_linebreak(character: char) -> bool {
    matches!(
        character,
        '\n' | '\u{b}'
            | '\u{c}'
            | '\r'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// A line-level port of `difflib.SequenceMatcher` with `isjunk = None`.
struct SequenceMatcher<'a> {
    a: Vec<&'a str>,
    b: Vec<&'a str>,
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> SequenceMatcher<'a> {
    /// Build a matcher over two line sequences.
    ///
    /// The `autojunk` heuristic is reproduced: from 200 lines on, a line that
    /// occurs more than `n / 100 + 1` times is dropped from the search index,
    /// exactly as `SequenceMatcher.__chain_b` drops "popular" elements.
    fn new(a: Vec<&'a str>, b: Vec<&'a str>) -> Self {
        let mut b2j: HashMap<&'a str, Vec<usize>> = HashMap::new();
        for (index, line) in b.iter().enumerate() {
            b2j.entry(line).or_default().push(index);
        }
        let n = b.len();
        if n >= 200 {
            let threshold = n / 100 + 1;
            b2j.retain(|_, indices| indices.len() <= threshold);
        }
        Self { a, b, b2j }
    }

    /// The longest matching block in `a[alo..ahi]` and `b[blo..bhi]`.
    ///
    /// Ties go to the earliest start in `a`, and then to the earliest start in
    /// `b`, matching `SequenceMatcher.find_longest_match`.
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(indices) = self.b2j.get(self.a[i]) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let previous = if j == 0 {
                        0
                    } else {
                        j2len.get(&(j - 1)).copied().unwrap_or(0)
                    };
                    let k = previous + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }

        // No junk function is configured, so the best block is only extended
        // over equal neighbours it could not reach through the index.
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// The monotonically increasing matching blocks, with the dummy tail.
    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let mut queue = vec![(0usize, self.a.len(), 0usize, self.b.len())];
        let mut blocks: Vec<(usize, usize, usize)> = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort();

        let (mut i1, mut j1, mut k1) = (0usize, 0usize, 0usize);
        let mut non_adjacent: Vec<(usize, usize, usize)> = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    non_adjacent.push((i1, j1, k1));
                }
                i1 = i2;
                j1 = j2;
                k1 = k2;
            }
        }
        if k1 > 0 {
            non_adjacent.push((i1, j1, k1));
        }
        non_adjacent.push((self.a.len(), self.b.len(), 0));
        non_adjacent
    }

    /// The operations that turn `a` into `b`.
    fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0usize, 0usize);
        let mut answer = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                answer.push(Opcode {
                    tag,
                    i1: i,
                    i2: ai,
                    j1: j,
                    j2: bj,
                });
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                answer.push(Opcode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: i,
                    j1: bj,
                    j2: j,
                });
            }
        }
        answer
    }

    /// Cluster the opcodes into hunks with `n` lines of context.
    fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(Opcode {
                tag: Tag::Equal,
                i1: 0,
                i2: 1,
                j1: 0,
                j2: 1,
            });
        }
        if codes[0].tag == Tag::Equal {
            let first = codes[0];
            codes[0].i1 = first.i1.max(first.i2.saturating_sub(n));
            codes[0].j1 = first.j1.max(first.j2.saturating_sub(n));
        }
        let last_index = codes.len() - 1;
        if codes[last_index].tag == Tag::Equal {
            let last = codes[last_index];
            codes[last_index].i2 = last.i2.min(last.i1 + n);
            codes[last_index].j2 = last.j2.min(last.j1 + n);
        }

        let window = n + n;
        let mut groups: Vec<Vec<Opcode>> = Vec::new();
        let mut group: Vec<Opcode> = Vec::new();
        for mut code in codes {
            if code.tag == Tag::Equal && code.i2 - code.i1 > window {
                group.push(Opcode {
                    tag: Tag::Equal,
                    i1: code.i1,
                    i2: code.i2.min(code.i1 + n),
                    j1: code.j1,
                    j2: code.j2.min(code.j1 + n),
                });
                groups.push(std::mem::take(&mut group));
                code.i1 = code.i1.max(code.i2.saturating_sub(n));
                code.j1 = code.j1.max(code.j2.saturating_sub(n));
            }
            group.push(code);
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].tag == Tag::Equal) {
            groups.push(group);
        }
        groups
    }
}
