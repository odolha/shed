//! Finding the link or file path under a cell, for Ctrl+click.

/// What a Ctrl+click on some text opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Url(String),
    /// A path as written; resolve it against the pane's directory.
    Path(String),
}

const SCHEMES: [&str; 6] = ["https://", "http://", "file://", "ftp://", "mailto:", "ssh://"];

/// The link in `line` that covers character `col`, if any.
pub fn link_at(line: &str, col: usize) -> Option<Link> {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }
    let breaks = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|');
    let start = (0..col).rev().find(|&i| breaks(chars[i])).map_or(0, |i| i + 1);
    let end = (col..chars.len()).find(|&i| breaks(chars[i])).unwrap_or(chars.len());
    let mut word: String = chars[start..end].iter().collect();

    // A URL may start partway through the word, as in `(see https://…)`.
    if let Some(at) = SCHEMES.iter().filter_map(|s| word.find(s)).min() {
        word = word[at..].to_string();
        trim_trailing(&mut word);
        return (word.len() > word.find(':').unwrap_or(0) + 3).then_some(Link::Url(word));
    }

    trim_trailing(&mut word);
    let word = word.trim_start_matches(['(', '[', '{']);
    // `path:line:col` from compilers: keep the path.
    let path = word.split(':').next().unwrap_or(word);
    let looks_like_path = path.contains('/') || path.starts_with('~') || path.starts_with('.');
    (looks_like_path && path.len() > 1).then(|| Link::Path(path.to_string()))
}

/// Drops punctuation that ends a sentence or closes a bracket the link did not open.
fn trim_trailing(word: &mut String) {
    while let Some(last) = word.chars().last() {
        let unbalanced = |open: char, close: char| last == close && word.matches(open).count() < word.matches(close).count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?')
            || unbalanced('(', ')')
            || unbalanced('[', ']')
            || unbalanced('{', '}')
        {
            word.pop();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let line = "see (https://example.com/a_(b)?q=1). done";
        assert_eq!(link_at(line, 10), Some(Link::Url("https://example.com/a_(b)?q=1".into())));
        assert_eq!(link_at(line, 0), None);
        assert_eq!(link_at("mailto:me@x.org", 3), Some(Link::Url("mailto:me@x.org".into())));
        assert_eq!(link_at("http://", 2), None);
    }

    #[test]
    fn paths() {
        assert_eq!(link_at("error at src/main.rs:12:5: bad", 12), Some(Link::Path("src/main.rs".into())));
        assert_eq!(link_at("edit ~/.bashrc now", 7), Some(Link::Path("~/.bashrc".into())));
        assert_eq!(link_at("plain words", 2), None);
    }
}
