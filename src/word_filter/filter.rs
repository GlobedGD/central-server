use std::{collections::HashSet, path::Path};

use aho_corasick::AhoCorasick;

use crate::word_filter::word_iterator::WordIterator;

pub struct WordFilter {
    algo: AhoCorasick,
    word_count: usize,
    words: Vec<String>,
    whole_words: HashSet<String>,
}

impl WordFilter {
    pub fn new(words: Vec<String>, whole_words: HashSet<String>) -> Self {
        Self {
            word_count: words.len() + whole_words.len(),
            algo: AhoCorasick::builder()
                .ascii_case_insensitive(true)
                .build(&words)
                .expect("failed to create word filter"),
            whole_words,
            words,
        }
    }

    pub fn new_from_lines(mut words: Vec<String>) -> Self {
        let mut whole_words = HashSet::new();

        words.retain_mut(|w| {
            let is_whole = w.starts_with("!!") && w.ends_with("!!") && w.len() > 4;

            if is_whole {
                let mut word = std::mem::take(w);
                word.remove_matches("!!");
                word.make_ascii_lowercase();
                whole_words.insert(word);
            }

            !is_whole && !w.is_empty()
        });

        Self::new(words, whole_words)
    }

    pub async fn new_from_path(p: &Path) -> Result<Self, std::io::Error> {
        let lines =
            tokio::fs::read_to_string(p).await?.lines().map(|x| x.to_string()).collect::<Vec<_>>();

        Ok(Self::new_from_lines(lines))
    }

    pub fn is_bad(&self, content: &str) -> Option<&str> {
        if let Some(x) = self.algo.find(content) {
            return Some(
                self.words.get(x.pattern().as_usize()).map_or("<unknown>", |x| x.as_str()),
            );
        }

        // check if any of the words are contained in self.whole_words
        let filter = |word: &str| {
            if word.len() > 2 {
                let word_lower = word.to_ascii_lowercase();
                self.whole_words.iter().find(|w| *w == &word_lower).map(|x| x.as_str())
            } else {
                None
            }
        };

        for word in WordIterator::new(content) {
            if let Some(bad) = filter(word) {
                return Some(bad);
            }
        }

        for word in content.split_whitespace() {
            if let Some(bad) = filter(word) {
                return Some(bad);
            }
        }

        None
    }

    pub fn word_count(&self) -> usize {
        self.word_count
    }
}

impl Default for WordFilter {
    fn default() -> Self {
        Self::new(Vec::new(), HashSet::new())
    }
}
