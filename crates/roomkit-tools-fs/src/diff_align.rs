//! Line alignment for diff and patch previews: a Myers diff bounded by the
//! work it may do.
//!
//! The work counted is what the comparison costs once the common first and
//! last lines are set aside: each remaining line examined, then each
//! furthest-reaching path the Myers search computes and each pair of equal
//! lines it follows; against an empty side there is no search. It grows
//! with the number of changed lines, not with the length of the files; the
//! trace kept to recover the edit script holds at most one entry per step.

use std::collections::HashSet;

use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

use crate::diff::DiffLineKind;
use crate::mutation;

/// Each line's kind in diff order, removals before additions within a
/// change, or `None` once the comparison would take more than `maximum`
/// steps. The script is a shortest one: as few lines removed and added as
/// any alignment allows.
pub(crate) fn align(
    old: &[&str],
    new: &[&str],
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Option<Vec<DiffLineKind>>, ToolError> {
    let (prefix, suffix) = common_ends(old, new);
    let (a, b) = middle(old, new, prefix, suffix);
    let Some(pairs) = common_lines(a, b, maximum, cancel)? else {
        return Ok(None);
    };
    let mut kinds = vec![DiffLineKind::Context; prefix];
    let (mut i, mut j) = (0, 0);
    for (x, y) in pairs {
        kinds.extend(std::iter::repeat_n(DiffLineKind::Removed, x - i));
        kinds.extend(std::iter::repeat_n(DiffLineKind::Added, y - j));
        kinds.push(DiffLineKind::Context);
        (i, j) = (x + 1, y + 1);
    }
    kinds.extend(std::iter::repeat_n(DiffLineKind::Removed, a.len() - i));
    kinds.extend(std::iter::repeat_n(DiffLineKind::Added, b.len() - j));
    kinds.extend(std::iter::repeat_n(DiffLineKind::Context, suffix));
    Ok(Some(kinds))
}

/// Each line's kind when the lines between the common ends are all
/// replaced: a valid but not always shortest script, at no search cost.
pub(crate) fn replace_middle(old: &[&str], new: &[&str]) -> Vec<DiffLineKind> {
    let (prefix, suffix) = common_ends(old, new);
    let (a, b) = middle(old, new, prefix, suffix);
    let mut kinds = vec![DiffLineKind::Context; prefix];
    kinds.extend(std::iter::repeat_n(DiffLineKind::Removed, a.len()));
    kinds.extend(std::iter::repeat_n(DiffLineKind::Added, b.len()));
    kinds.extend(std::iter::repeat_n(DiffLineKind::Context, suffix));
    kinds
}

/// The numbers of equal lines at the start and, after them, at the end.
fn common_ends(old: &[&str], new: &[&str]) -> (usize, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let rest = |lines: &[&str]| lines.len() - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(rest(old).min(rest(new)))
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, suffix)
}

/// The lines between the common ends.
fn middle<'a, 'b>(
    old: &'a [&'b str],
    new: &'a [&'b str],
    prefix: usize,
    suffix: usize,
) -> (&'a [&'b str], &'a [&'b str]) {
    let cut = |lines: &'a [&'b str]| lines.get(prefix..lines.len() - suffix).unwrap_or_default();
    (cut(old), cut(new))
}

/// The pairs of equal lines, `(index in a, index in b)` in order, of a
/// longest common subsequence, or `None` past `maximum` steps. A line found
/// on one side only cannot be in it, so the search runs without them.
fn common_lines(
    a: &[&str],
    b: &[&str],
    maximum: usize,
    cancel: &CancellationToken,
) -> Result<Option<Vec<(usize, usize)>>, ToolError> {
    // Against an empty side every line is removed or added: nothing to search.
    if a.is_empty() || b.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(left) = maximum.checked_sub(a.len() + b.len()) else {
        return Ok(None);
    };
    let in_a: HashSet<&str> = a.iter().copied().collect();
    let in_b: HashSet<&str> = b.iter().copied().collect();
    let (kept_a, lines_a) = kept(a, &in_b);
    let (kept_b, lines_b) = kept(b, &in_a);
    let Some(pairs) = Search::new(&lines_a, &lines_b, left, cancel).run()? else {
        return Ok(None);
    };
    let original = |kept: &[usize], index: usize| kept.get(index).copied();
    pairs
        .into_iter()
        .map(|(x, y)| Some((original(&kept_a, x)?, original(&kept_b, y)?)))
        .collect::<Option<Vec<_>>>()
        .map(Some)
        .ok_or_else(|| ToolError::InvalidInput("invalid diff alignment".into()))
}

/// The indices and lines of `lines` that `other` also holds.
fn kept<'a>(lines: &[&'a str], other: &HashSet<&str>) -> (Vec<usize>, Vec<&'a str>) {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| other.contains(*line))
        .map(|(index, line)| (index, *line))
        .unzip()
}

/// Myers's greedy search for a shortest edit script of `a` into `b`.
struct Search<'s, 'l> {
    a: &'s [&'l str],
    b: &'s [&'l str],
    maximum: usize,
    work: usize,
    cancel: &'s CancellationToken,
}

// Every index below is derived from the lengths of `a` and `b`: a diagonal
// `k` of step `d` lies in `-d..=d`, stored at `offset + k` in a vector of
// `2 * (n + m) + 3` entries, and at `(k + d) / 2` in the step's trace.
#[expect(
    clippy::indexing_slicing,
    reason = "diagonal indices are bounded by the checked step and lengths"
)]
impl<'s, 'l> Search<'s, 'l> {
    fn new(
        a: &'s [&'l str],
        b: &'s [&'l str],
        maximum: usize,
        cancel: &'s CancellationToken,
    ) -> Self {
        Self {
            a,
            b,
            maximum,
            work: 0,
            cancel,
        }
    }

    /// Count `steps` of work; false once past the maximum.
    fn spend(&mut self, steps: usize) -> Result<bool, ToolError> {
        let before = self.work / 4096;
        self.work = self.work.saturating_add(steps);
        if self.work / 4096 != before {
            mutation::check_cancel(self.cancel)?;
        }
        Ok(self.work <= self.maximum)
    }

    /// The common pairs of a shortest edit script, or `None` past the
    /// maximum. For each step `d`, the furthest point reached on each
    /// diagonal is kept, to walk the script back from the end.
    fn run(mut self) -> Result<Option<Vec<(usize, usize)>>, ToolError> {
        let (n, m) = (self.a.len(), self.b.len());
        let offset = n + m + 1;
        let mut furthest = vec![0_usize; 2 * (n + m) + 3];
        let mut trace: Vec<Vec<u32>> = Vec::new();
        for d in 0..=n + m {
            mutation::check_cancel(self.cancel)?;
            if !self.spend(d + 1)? {
                return Ok(None);
            }
            let mut step = Vec::with_capacity(d + 1);
            for j in 0..=d {
                let index = offset - d + 2 * j;
                let down = j == 0 || (j != d && furthest[index - 1] < furthest[index + 1]);
                let mut x = if down {
                    furthest[index + 1]
                } else {
                    furthest[index - 1] + 1
                };
                let mut y = x + offset - index;
                while x < n && y < m && self.a[x] == self.b[y] {
                    if !self.spend(1)? {
                        return Ok(None);
                    }
                    (x, y) = (x + 1, y + 1);
                }
                furthest[index] = x;
                if x >= n && y >= m {
                    return Ok(Some(back(&trace, (n, m))));
                }
                let Ok(x) = u32::try_from(x) else {
                    return Ok(None);
                };
                step.push(x);
            }
            trace.push(step);
        }
        Ok(None)
    }
}

/// Walk a search's `trace` back from `(n, m)`: the pairs of equal lines its
/// shortest path follows, in order.
#[expect(
    clippy::indexing_slicing,
    reason = "trace indices follow the diagonals the search computed"
)]
fn back(trace: &[Vec<u32>], end: (usize, usize)) -> Vec<(usize, usize)> {
    let (mut x, mut y) = end;
    let mut pairs = Vec::new();
    for d in (1..=trace.len()).rev() {
        let previous = &trace[d - 1];
        let diagonal = (x + d - y) / 2;
        let down = diagonal == 0 || (diagonal != d && previous[diagonal - 1] < previous[diagonal]);
        let start = if down {
            previous[diagonal] as usize
        } else {
            previous[diagonal - 1] as usize + 1
        };
        while x > start {
            (x, y) = (x - 1, y - 1);
            pairs.push((x, y));
        }
        if down {
            y -= 1;
        } else {
            x -= 1;
        }
    }
    while x > 0 {
        (x, y) = (x - 1, y - 1);
        pairs.push((x, y));
    }
    pairs.reverse();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The length of a longest common subsequence, by the quadratic table.
    fn lcs(a: &[&str], b: &[&str]) -> usize {
        let mut table = vec![vec![0; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                table[i][j] = if a[i] == b[j] {
                    table[i + 1][j + 1] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        table[0][0]
    }

    /// Every sequence of up to `length` lines over `alphabet`.
    fn sequences(alphabet: &[&'static str], length: usize) -> Vec<Vec<&'static str>> {
        let mut all = vec![Vec::new()];
        let mut last = vec![Vec::new()];
        for _ in 0..length {
            last = last
                .iter()
                .flat_map(|prefix: &Vec<&str>| {
                    alphabet.iter().map(move |line| {
                        let mut longer = prefix.clone();
                        longer.push(*line);
                        longer
                    })
                })
                .collect();
            all.extend(last.iter().cloned());
        }
        all
    }

    #[test]
    fn the_script_is_a_shortest_one_and_rebuilds_both_sides() {
        let cancel = CancellationToken::new();
        let all = sequences(&["a", "b", "c"], 5);
        for old in &all {
            for new in &all {
                let kinds = align(old, new, usize::MAX, &cancel).unwrap().unwrap();
                let (mut i, mut j) = (0, 0);
                for kind in &kinds {
                    match kind {
                        DiffLineKind::Context => {
                            assert_eq!(old[i], new[j], "{old:?} {new:?}");
                            (i, j) = (i + 1, j + 1);
                        }
                        DiffLineKind::Removed => i += 1,
                        DiffLineKind::Added => j += 1,
                    }
                }
                assert_eq!((i, j), (old.len(), new.len()), "{old:?} {new:?}");
                let kept = kinds
                    .iter()
                    .filter(|k| **k == DiffLineKind::Context)
                    .count();
                assert_eq!(kept, lcs(old, new), "{old:?} {new:?}");
            }
        }
    }

    #[test]
    fn removals_come_before_additions_and_the_work_is_bounded() {
        let cancel = CancellationToken::new();
        let kinds = align(&["x", "a", "y"], &["x", "b", "y"], 100, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(
            kinds,
            [
                DiffLineKind::Context,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Context
            ]
        );
        // Reversed lines are all common: the search costs about (n + m)².
        let old: Vec<String> = (0..100).map(|i| i.to_string()).collect();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = old.iter().rev().copied().collect();
        assert_eq!(align(&old, &new, 5_000, &cancel).unwrap(), None);
        assert!(align(&old, &new, 50_000, &cancel).unwrap().is_some());
        let replaced = replace_middle(&old, &new);
        assert_eq!(replaced.len(), 200);
        assert!(replaced[..100].iter().all(|k| *k == DiffLineKind::Removed));
    }
}
