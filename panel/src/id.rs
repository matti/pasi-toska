//! Browser ids: 6 characters of [a-z0-9], used in pod names and URLs.

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const LEN: usize = 6;

/// A fresh random id for which `taken` returns false.
pub fn generate(taken: impl Fn(&str) -> bool) -> String {
    loop {
        let id = build(|n| rand::random_range(0..n));
        if !taken(&id) {
            return id;
        }
    }
}

fn build(mut pick: impl FnMut(usize) -> usize) -> String {
    (0..LEN).map(|_| ALPHABET[pick(ALPHABET.len())] as char).collect()
}

pub fn is_valid(id: &str) -> bool {
    id.len() == LEN && id.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_valid_and_avoid_taken() {
        let first = generate(|_| false);
        assert!(is_valid(&first), "{first}");
        for _ in 0..200 {
            let id = generate(|s| s == first);
            assert!(is_valid(&id));
            assert_ne!(id, first);
        }
    }

    #[test]
    fn build_uses_the_whole_alphabet() {
        let mut i = 0;
        assert_eq!(build(|_| { i += 7; i % 36 }), "hov29g");
        assert_eq!(build(|n| n - 1), "999999");
    }

    #[test]
    fn validation() {
        assert!(is_valid("ab12z9"));
        for bad in ["", "abc", "abcdefg", "ABC123", "ab-123", "ab/../"] {
            assert!(!is_valid(bad), "{bad}");
        }
    }
}
