use jellyfin_naming::{EpisodeExpression, NamingOptions};

#[test]
fn naming_options_compile() {
    let options = NamingOptions::default();

    assert!(!options.clean_date_time_regexes.is_empty());
    assert!(!options.clean_string_regexes.is_empty());
}

#[test]
fn naming_options_episode_expressions() {
    let mut expression = EpisodeExpression::try_new("", false).expect("empty regex is valid");

    assert!(!expression.is_optimistic);
    expression.is_optimistic = true;
    assert!(expression.is_optimistic);

    assert_eq!(expression.expression(), "");
    assert!(expression.regex().is_match("anything"));
    expression
        .set_expression("test")
        .expect("replacement regex is valid");
    assert_eq!(expression.expression(), "test");
    assert!(expression.regex().is_match("TEST"));
    assert!(!expression.regex().is_match("anything"));
}

#[test]
fn invalid_user_expression_preserves_compiled_state() {
    let mut expression = EpisodeExpression::try_new("test", false).expect("valid regex");

    assert!(EpisodeExpression::try_new("(", false).is_err());
    assert!(expression.set_expression("(").is_err());
    assert_eq!(expression.expression(), "test");
    assert!(expression.regex().is_match("TEST"));
}
