pub struct WordIterator<'a> {
    remainder: &'a str,
}

impl<'a> WordIterator<'a> {
    pub fn new(s: &'a str) -> Self {
        Self { remainder: s.trim_start() }
    }
}

impl<'a> Iterator for WordIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remainder.is_empty() {
            return None;
        }

        let mut chars = self.remainder.char_indices();
        // skip first letter (possibly capital)
        chars.next();

        let mut break_index = self.remainder.len();

        for (byte_idx, c) in chars {
            // whitespace - end of word
            if c.is_whitespace() {
                break_index = byte_idx;
                break;
            }
            // uppercase - end of word
            if c.is_uppercase() {
                break_index = byte_idx;
                break;
            }
        }

        let word = &self.remainder[..break_index];

        // update the remainder
        self.remainder = self.remainder[break_index..].trim_start();

        Some(word)
    }
}
