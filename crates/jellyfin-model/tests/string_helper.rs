use jellyfin_model::first_to_upper;

#[test]
fn official_first_to_upper_matrix() {
    for (input, expected) in [
        ("", ""),
        ("banana", "Banana"),
        ("Banana", "Banana"),
        ("ä", "Ä"),
        ("\u{0017}", "\u{0017}"),
    ] {
        assert_eq!(first_to_upper(input), expected, "{input:?}");
    }
}

#[test]
fn first_to_upper_only_changes_the_first_utf16_character() {
    let mut random = TestRandom::new(0x4a65_6c6c_7966_696e);
    let alphabet = [
        'a', 'z', 'A', 'Z', 'ä', 'ö', 'ı', 'ß', 'ǰ', '1', '-', '\u{0017}', '中', '𐐨',
    ];

    for _ in 0..10_000 {
        let length = random.next_usize(32) + 1;
        let input = (0..length)
            .map(|_| alphabet[random.next_usize(alphabet.len())])
            .collect::<String>();
        let result = first_to_upper(&input);
        let input_first = input.chars().next().unwrap();
        let result_first = result.chars().next().unwrap();

        let uppercase = input_first.to_uppercase().collect::<Vec<_>>();
        let has_single_utf16_mapping =
            input_first.len_utf16() == 1 && uppercase.len() == 1 && uppercase[0].len_utf16() == 1;
        if has_single_utf16_mapping {
            assert!(!result_first.is_lowercase(), "{input:?} -> {result:?}");
        } else {
            assert_eq!(result, input, "{input:?} -> {result:?}");
        }
        assert_eq!(
            &result[result_first.len_utf8()..],
            &input[input_first.len_utf8()..],
            "{input:?} -> {result:?}"
        );
    }
}

struct TestRandom(u64);

impl TestRandom {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_usize(&mut self, upper_bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as usize % upper_bound
    }
}
