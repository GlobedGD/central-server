use std::{collections::HashSet, path::Path};

use aho_corasick::AhoCorasick;

pub struct WordFilter {
    algo: AhoCorasick,
    word_count: usize,
    whole_words: HashSet<String>,
}

impl WordFilter {
    pub fn new(words: &[String], whole_words: HashSet<String>) -> Self {
        Self {
            word_count: words.len() + whole_words.len(),
            algo: AhoCorasick::builder()
                .ascii_case_insensitive(true)
                .build(words)
                .expect("failed to create word filter"),
            whole_words,
        }
    }

    pub fn new_from_lines(mut words: Vec<String>) -> Self {
        let mut whole_words = HashSet::new();

        words.retain_mut(|w| {
            let is_whole = w.starts_with("!!") && w.ends_with("!!") && w.len() > 4;

            if is_whole {
                let mut word = std::mem::take(w);
                word.remove_matches("!!");
                whole_words.insert(word);
            }

            !is_whole && !w.is_empty()
        });

        Self::new(&words, whole_words)
    }

    pub async fn new_from_path(p: &Path) -> Result<Self, std::io::Error> {
        let lines =
            tokio::fs::read_to_string(p).await?.lines().map(|x| x.to_string()).collect::<Vec<_>>();

        Ok(Self::new_from_lines(lines))
    }

    pub fn is_bad(&self, content: &str) -> bool {
        if self.algo.find(content).is_some() {
            return true;
        }

        // check if any of the words are contained in self.whole_words
        content.split(' ').any(|word| self.whole_words.contains(word))
    }

    pub async fn reload_from_file(&mut self, path: &Path) -> Result<(), std::io::Error> {
        let lines = tokio::fs::read_to_string(path)
            .await?
            .lines()
            .map(|x| x.to_string())
            .collect::<Vec<_>>();

        let new_filter = Self::new_from_lines(lines);
        self.algo = new_filter.algo;
        self.word_count = new_filter.word_count;
        self.whole_words = new_filter.whole_words;

        Ok(())
    }

    pub fn word_count(&self) -> usize {
        self.word_count
    }
}

impl Default for WordFilter {
    fn default() -> Self {
        Self::new(&[], HashSet::new())
    }
}
