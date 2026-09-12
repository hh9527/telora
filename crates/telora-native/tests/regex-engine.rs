//! Isolated engine experiment. Production continues to use meta::Regex.
use regex_automata::{Input, meta::Regex, nfa::thompson::pikevm::PikeVM};

#[test]
fn pikevm_matches_meta_ranges_and_capture_slots() {
    // Include ParseBy patterns and boundary cases used by native language assets.
    let patterns = [
        r"^(?P<a>[0-9]+)(?:-(?P<b>[a-z]+))?$",
        r"^(?P<host>[^:]+):(?P<port>-?\d+)(?:#(?P<label>.*))?$",
        r"^(?P<name>\w+)@(?P<endpoint>.+)$",
        r"(?P<word>[a-z]+)", r"(?P<word>\w+)", r"\b\w+\b",
        r"(?P<a>a|ab)(?P<b>b?)", r"(?P<a>a+?)(?P<b>a*)",
        r"(?m)^(?P<line>.*)$", r"(?s)(?P<all>.*)",
        r"(?P<empty>)(?P<optional>x)?", r"(?P<unicode>\p{Greek}+)",
        r"(?:a?){16}a{16}", r"(?P<nested>(?P<inner>a)+)?",
    ];
    let inputs = ["", "42", "42-name", "-42", "host:42", "host:-42#label",
        "name@host:42", "wrong", "a", "ab", "aaaa", "a\nb\n", "文本", "αβγ",
        "xαβγy", "é", "a💡b", " foo bar ", "aaaaaaaaaaaaaaaa"];
    let mut comparisons = 0;
    for pattern in patterns {
        let meta = Regex::new(pattern).unwrap();
        let pike = PikeVM::new(pattern).unwrap();
        let mut meta_cache = meta.create_cache();
        let mut pike_cache = pike.create_cache();
        let mut meta_captures = meta.create_captures();
        let mut pike_captures = pike.create_captures();
        for text in inputs {
            // Preserve surrounding context when searching a subrange, including
            // empty ranges and positions inside a UTF-8 code point.
            for range in [0..text.len(), 0..0, text.len()..text.len(), text.len().min(1)..text.len()] {
                let input = Input::new(text).range(range.clone());
                meta.search_captures_with(&mut meta_cache, &input, &mut meta_captures);
                pike.search(&mut pike_cache, &input, &mut pike_captures);
                assert_eq!(meta_captures.get_match(), pike_captures.get_match(), "{pattern:?} {text:?} {range:?}");
                if meta_captures.is_match() {
                    assert_eq!(meta_captures.slots(), pike_captures.slots(), "{pattern:?} {text:?} {range:?}");
                }
                let earliest = input.earliest(true);
                let expected = meta.search_with(&mut meta_cache, &earliest).is_some();
                pike.search(&mut pike_cache, &earliest, &mut pike_captures);
                assert_eq!(expected, pike_captures.is_match(), "earliest {pattern:?} {text:?} {range:?}");
                comparisons += 1;
            }
        }
    }
    assert_eq!(comparisons, 1064);
}

#[test]
fn pikevm_cache_observations_across_input_lengths() {
    for pattern in [r"(?P<word>\w+)", r"(?:a?){32}a{32}", r"(?P<a>a*)(?P<b>b*)"] {
        let pike = PikeVM::new(pattern).unwrap();
        let mut cache = pike.create_cache();
        let mut captures = pike.create_captures();
        let initial = cache.memory_usage();
        pike.search(&mut cache, &Input::new("ab"), &mut captures);
        let warm = cache.memory_usage();
        pike.search(&mut cache, &Input::new(&"a".repeat(65_536)), &mut captures);
        let large = cache.memory_usage();
        eprintln!("pattern={pattern:?} states={} slots={} cache_bytes={initial}/{warm}/{large}",
            pike.get_nfa().states().len(), captures.slots().len());
        assert_eq!(warm, large, "cache growth depends on input length for {pattern:?}");
    }
}
