//! Code read aloud (#295): the announcement, line labels, symbols in
//! words, identifiers split into words, and the pauses (commas) between the
//! parts of a line.
use sonara_core::code_speech::{announcement, end_of_code, inline, language_name, line};

#[test]
fn the_fence_info_string_is_spoken_as_a_language_name() {
    assert_eq!(language_name("ts").as_deref(), Some("TypeScript"));
    assert_eq!(language_name("TSX").as_deref(), Some("T S X"));
    assert_eq!(language_name("py title=x.py").as_deref(), Some("Python"));
    assert_eq!(language_name("bash").as_deref(), Some("Bash"));
    assert_eq!(
        language_name("elm").as_deref(),
        Some("elm"),
        "unknown: as written"
    );
    assert_eq!(language_name(""), None);
}

#[test]
fn a_block_is_announced_with_its_language_and_size_and_closed_by_name() {
    assert_eq!(announcement("ts", 7), "TypeScript code block, 7 lines.");
    assert_eq!(announcement("", 1), "Code block, 1 line.");
    assert_eq!(end_of_code("ts"), "End of TypeScript code.");
    assert_eq!(end_of_code(""), "End of code.");
}

#[test]
fn the_lesson_lines_are_read_with_labels_and_named_symbols() {
    // The React lesson that was only announced (#290).
    assert_eq!(
        line("function Welcome() {", "jsx", 1).as_deref(),
        Some("Line 1. function Welcome, empty parens, open brace.")
    );
    assert_eq!(
        line("  return <h1>Hei, verden!</h1>;", "jsx", 2).as_deref(),
        Some("Line 2. return, open tag h1, Hei, verden!, close tag h1.")
    );
    assert_eq!(line("}", "jsx", 3).as_deref(), Some("Line 3. close brace."));
    assert_eq!(
        line("      <Welcome />", "jsx", 4).as_deref(),
        Some("Line 4. tag Welcome, self-closing.")
    );
    assert_eq!(
        line("    <div>", "jsx", 5).as_deref(),
        Some("Line 5. open tag div.")
    );
}

#[test]
fn every_symbol_has_a_word_and_dots_keep_a_name_together() {
    assert_eq!(
        line(
            r#"const greet = (name) => console.log("Hi, " + name);"#,
            "js",
            3
        )
        .as_deref(),
        Some(
            "Line 3. const greet, equals, open paren, name, close paren, arrow, \
             console dot log, open paren, quote Hi, end quote, plus, name, close paren."
        )
    );
    assert_eq!(
        line("if a < b and c != d:", "python", 1).as_deref(),
        Some("Line 1. if a, less than, b and c, not equals, d, colon.")
    );
    assert_eq!(
        line("x = items[0] * 2 % 3;  y += 1;", "js", 2).as_deref(),
        Some(
            "Line 2. x, equals, items, open bracket, 0, close bracket, star, 2, percent, 3, \
             semicolon, y, plus equals, 1."
        )
    );
}

#[test]
fn identifiers_are_split_into_words() {
    assert_eq!(
        line("getUserName(user_id)", "js", 1).as_deref(),
        Some("Line 1. get user name, open paren, user id, close paren.")
    );
    assert_eq!(
        line("HTTPServer.listen()", "ts", 1).as_deref(),
        Some("Line 1. HTTP server dot listen, empty parens.")
    );
}

#[test]
fn comments_and_strings_are_read_as_words() {
    assert_eq!(
        line("// say hi to the user", "js", 1).as_deref(),
        Some("Line 1. comment, say hi to the user.")
    );
    assert_eq!(
        line("print('hi')  # greet", "python", 4).as_deref(),
        Some("Line 4. print, open paren, quote hi end quote, close paren, comment, greet.")
    );
    assert_eq!(
        line("let s = `total: ${n}`;", "ts", 1).as_deref(),
        Some("Line 1. let s, equals, backtick total: insert n end backtick.")
    );
}

#[test]
fn shell_flags_are_dashes_not_minus() {
    assert_eq!(
        line("npm install --save-dev typescript -g", "bash", 1).as_deref(),
        Some("Line 1. npm install, dash dash save dev, typescript, dash g.")
    );
    assert_eq!(
        line("a - b", "js", 1).as_deref(),
        Some("Line 1. a, minus, b.")
    );
}

#[test]
fn a_blank_or_bare_semicolon_line_is_not_spoken() {
    assert_eq!(line("   ", "js", 1), None);
    assert_eq!(line(";", "js", 1), None);
}

#[test]
fn inline_code_uses_the_same_words_without_a_label() {
    assert_eq!(inline("<Welcome />"), "tag Welcome, self-closing");
    assert_eq!(inline("useState()"), "use state, empty parens");
    assert_eq!(inline("sonara.exe"), "sonara dot exe");
    assert_eq!(inline("--verbose"), "dash dash verbose");
    assert_eq!(inline("npm"), "npm");
}

#[test]
fn a_url_is_never_read_as_a_comment() {
    let l = line("curl -fsSL https://example.com/install.sh | sh", "bash", 1).unwrap();
    assert!(!l.contains("comment"), "{l}");
    assert!(l.contains("pipe"), "{l}");
    // Inline, a URL is left to the prose rules.
    assert_eq!(inline("https://example.com"), "https://example.com");
}

#[test]
fn hyphenated_names_are_words_not_minus() {
    assert_eq!(inline("sonara-core"), "sonara core");
    assert_eq!(
        inline("cargo test -p sonara-core"),
        "cargo test, dash p, sonara core"
    );
    assert_eq!(
        line("font-size: 12px;", "css", 1).as_deref(),
        Some("Line 1. font size, colon, 12px.")
    );
    // A date inline is left whole for the prose rules.
    assert_eq!(inline("2026-10-13"), "2026-10-13");
}

#[test]
fn generics_are_not_tags() {
    assert_eq!(
        inline("Vec<String>"),
        "Vec, less than, String, greater than"
    );
    let l = line("const [n, setN] = useState<number>(0);", "tsx", 1).unwrap();
    assert!(!l.contains("tag") && !l.contains("(0)"), "{l}");
    let l = line("for (let i = 0; i<n; i++) {", "jsx", 1).unwrap();
    assert!(!l.contains("tag"), "{l}");
    // Real JSX is still read as tags.
    assert_eq!(
        line("return (<App />)", "jsx", 1).as_deref(),
        Some("Line 1. return, open paren, tag App, self-closing, close paren.")
    );
}

#[test]
fn apostrophes_and_lifetimes_are_not_strings() {
    let l = line("fn get<'a>(&'a self) -> &'a str {", "rust", 1).unwrap();
    assert!(!l.contains("quote"), "{l}");
    assert!(l.contains("lifetime a"), "{l}");
    assert_eq!(
        line("Don't do this, it's bad.", "text", 1).as_deref(),
        Some("Line 1. Don't do this, it's bad.")
    );
}

#[test]
fn globs_are_not_block_comments_and_real_ones_are() {
    let l = line("ls src/*.rs", "sh", 1).unwrap();
    assert!(!l.contains("comment"), "{l}");
    assert!(!inline("src/**/*.ts").contains("comment"));
    assert_eq!(
        line("int x = 1; /* one */", "c", 1).as_deref(),
        Some("Line 1. int x, equals, 1, semicolon, comment, one.")
    );
}

#[test]
fn jsx_text_after_an_expression_is_read_as_words() {
    assert_eq!(
        line("<p>Hello, {name}!</p>", "jsx", 1).as_deref(),
        Some("Line 1. open tag p, Hello, open brace, name, close brace, !, close tag p.")
    );
}

#[test]
fn a_number_before_a_multibyte_character_does_not_panic() {
    // #294 review: `90°` in inline or fenced code.
    assert!(inline("90°").contains("90"));
    assert!(line("x = 5€", "js", 1).unwrap().contains('5'));
    assert!(inline("9é").contains('9'));
}

#[test]
fn decomposed_accents_stay_in_their_word() {
    let l = line("let cafe\u{301} = 1", "rust", 1).unwrap();
    assert!(!l.contains(" \u{301}"), "{l}");
    assert!(l.contains("cafe\u{301}"), "{l}");
}
