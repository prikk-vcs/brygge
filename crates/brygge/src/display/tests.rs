use super::*;

#[test]
fn human_leaves_plain_text_untouched_and_borrows() {
    let s = "a normal commit message, with punctuation! 日本語 too.";
    let out = human(s);
    assert_eq!(out, s);
    assert!(
        matches!(out, Cow::Borrowed(_)),
        "no escaping needed, no allocation"
    );
}

#[test]
fn human_escapes_ansi_escape_sequences() {
    // \x1b[2J is a terminal-clear sequence; must never reach the terminal raw.
    let out = human("clear: \x1b[2J done");
    assert_eq!(out, "clear: \\u{001b}[2J done");
}

#[test]
fn human_escapes_bidi_and_invisible_format_characters() {
    // U+202E RIGHT-TO-LEFT OVERRIDE can visually reorder following text.
    let out = human("safe\u{202E}evil");
    assert_eq!(out, "safe\\u{202e}evil");
    // U+200B ZERO WIDTH SPACE and U+FEFF BOM are invisible.
    let out = human("a\u{200B}b\u{FEFF}c");
    assert_eq!(out, "a\\u{200b}b\\u{feff}c");
}

#[test]
fn human_escapes_the_review_003_r2_extension_set() {
    // Soft hyphen: invisible.
    assert_eq!(human("a\u{00AD}b"), "a\\u{00ad}b");
    // Arabic letter mark: a bidi control.
    assert_eq!(human("a\u{061C}b"), "a\\u{061c}b");
    // Mongolian vowel separator: invisible.
    assert_eq!(human("a\u{180E}b"), "a\\u{180e}b");
    // Line and paragraph separators: can forge a line in output that is otherwise one line per record.
    assert_eq!(human("a\u{2028}b"), "a\\u{2028}b");
    assert_eq!(human("a\u{2029}b"), "a\\u{2029}b");
    // A deprecated format control in the 206A-206F block.
    assert_eq!(human("a\u{206F}b"), "a\\u{206f}b");
    // An interlinear annotation control.
    assert_eq!(human("a\u{FFF9}b"), "a\\u{fff9}b");
}

#[test]
fn human_escapes_c0_and_c1_controls() {
    assert_eq!(human("\x00"), "\\u{0000}");
    assert_eq!(human("\x1f"), "\\u{001f}");
    assert_eq!(human("\x7f"), "\\u{007f}");
    assert_eq!(human("\u{80}"), "\\u{0080}");
    assert_eq!(human("\u{9f}"), "\\u{009f}");
    // The boundaries just outside the escaped ranges pass through.
    assert_eq!(human("\x20"), "\x20");
    assert_eq!(human("\u{a0}"), "\u{a0}");
}

#[test]
fn human_doubles_a_literal_backslash_so_output_escapes_cannot_be_imitated() {
    // Input containing a literal `\u{202e}`-looking sequence, typed as plain characters (backslash, u,
    // brace, digits, brace) must not be indistinguishable from this function's own escaping.
    let out = human("literal \\u{202e} text");
    assert_eq!(out, "literal \\\\u{202e} text");
    // A bare backslash alone.
    assert_eq!(human("a\\b"), "a\\\\b");
}

#[test]
fn human_escapes_mixed_content_once_each() {
    let out = human("msg\x1b[31mred\x1b[0m and \\backslash");
    assert_eq!(out, "msg\\u{001b}[31mred\\u{001b}[0m and \\\\backslash");
}

#[test]
fn machine_value_passes_through_the_safe_set() {
    let safe = "AZaz09-._~/@:+";
    assert_eq!(machine_value(safe), safe);
}

#[test]
fn machine_value_percent_encodes_equals_space_percent_and_newline() {
    assert_eq!(machine_value("a=b"), "a%3Db");
    assert_eq!(machine_value("a b"), "a%20b");
    assert_eq!(machine_value("100%"), "100%25");
    assert_eq!(machine_value("a\nb"), "a%0Ab");
    assert_eq!(machine_value("a\rb"), "a%0Db");
}

#[test]
fn machine_value_cannot_produce_a_bare_equals_or_newline() {
    let inputs = ["=", "\n", "a=b\nc=d", "%3D looks encoded already"];
    for s in inputs {
        let encoded = machine_value(s);
        assert!(
            !encoded.contains('='),
            "encoded {s:?} -> {encoded:?} contains ="
        );
        assert!(
            !encoded.contains('\n'),
            "encoded {s:?} -> {encoded:?} contains a newline"
        );
    }
}

#[test]
fn machine_value_percent_encodes_non_ascii_bytes() {
    // "é" is 2 UTF-8 bytes: 0xC3 0xA9.
    assert_eq!(machine_value("é"), "%C3%A9");
}

#[test]
fn machine_value_of_a_ref_name_with_equals_matches_the_handoff_example() {
    assert_eq!(machine_value("a=b"), "a%3Db");
}
