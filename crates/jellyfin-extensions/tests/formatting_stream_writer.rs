use jellyfin_extensions::{FormattingStreamWriter, escape_concat_file_path};

#[test]
fn writes_float_with_invariant_decimal_separator() {
    let mut writer = FormattingStreamWriter::new(Vec::new());

    writer
        .write_format(format_args!("{}", std::f64::consts::PI))
        .expect("write must succeed");

    assert_eq!(writer.into_inner(), b"3.141592653589793");
}

#[test]
fn writes_concat_entries_with_official_single_quote_escaping() {
    let mut writer = FormattingStreamWriter::new(Vec::new());
    let path = escape_concat_file_path("/media/O'Brien/title01.vob");

    writer
        .write_format_line(format_args!("file '{path}'"))
        .expect("file stanza must write");
    writer
        .write_format_line(format_args!("duration {}", 12.5))
        .expect("duration stanza must write");

    assert_eq!(
        String::from_utf8(writer.into_inner()).expect("output must be UTF-8"),
        "file '/media/O'\\''Brien/title01.vob'\nduration 12.5\n"
    );
}
