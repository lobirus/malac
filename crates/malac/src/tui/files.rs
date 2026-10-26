//! Fuzzy matching for the `@` picker and the folder browser. The files
//! themselves come from [`crate::files::index`], here or on the daemon's
//! machine.

/// A candidate that matched, with the characters that matched it.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub path: String,
    pub score: i64,
    /// Indices of matched characters, for highlighting.
    pub positions: Vec<usize>,
}

fn boundary(previous: Option<char>, c: char) -> bool {
    match previous {
        None => true,
        Some(p) => {
            matches!(p, '/' | '_' | '-' | '.' | ' ') || (p.is_lowercase() && c.is_uppercase())
        }
    }
}

/// Scores `candidate` against `query`: the query's characters must appear
/// in order, ignoring case. Runs, word starts and the file name score
/// higher, gaps and long paths lower.
pub fn score(query: &str, candidate: &str) -> Option<(i64, Vec<usize>)> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let name_start = chars.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let mut best: Option<(i64, Vec<usize>)> = None;
    // Try each place the first character occurs; take the best greedy alignment.
    for start in (0..lower.len()).filter(|&i| lower[i] == query[0]) {
        let mut positions = vec![start];
        let mut at = start + 1;
        for &q in &query[1..] {
            match lower[at..].iter().position(|&c| c == q) {
                Some(offset) => {
                    positions.push(at + offset);
                    at += offset + 1;
                }
                None => break,
            }
        }
        if positions.len() < query.len() {
            break;
        }
        let mut total = 0i64;
        let mut previous: Option<usize> = None;
        for &p in &positions {
            let before = p.checked_sub(1).map(|i| chars[i]);
            if boundary(before, chars[p]) {
                total += 10;
            }
            if p >= name_start {
                total += 4;
            }
            match previous {
                Some(q) if q + 1 == p => total += 8,
                Some(q) => total -= ((p - q - 1) as i64).min(6),
                None => {}
            }
            previous = Some(p);
        }
        total -= chars.len() as i64 / 8;
        if best.as_ref().is_none_or(|(b, _)| total > *b) {
            best = Some((total, positions));
        }
    }
    best
}

/// The best matches for `query`, at most `limit` of them.
pub fn best(files: &[String], query: &str, limit: usize) -> Vec<Match> {
    let mut matches: Vec<Match> = files
        .iter()
        .filter_map(|path| {
            let (score, positions) = score(query, path)?;
            Some(Match {
                path: path.clone(),
                score,
                positions,
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.path.len().cmp(&b.path.len()))
            .then(a.path.cmp(&b.path))
    });
    matches.truncate(limit);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn prefers_file_names_runs_and_word_starts() {
        let files = paths(&[
            "src/domain/main_test.rs",
            "src/main.rs",
            "docs/maintenance.md",
            "README.md",
        ]);
        let found: Vec<String> = best(&files, "main", 10)
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(found[0], "src/main.rs");
        assert!(!found.contains(&"README.md".to_owned()));
        let found = best(&files, "smr", 10);
        assert_eq!(found[0].path, "src/main.rs");
        assert_eq!(found[0].positions, vec![0, 4, 9]);
        assert!(score("xyz", "src/main.rs").is_none());
        assert_eq!(best(&files, "", 2).len(), 2);
    }
}
