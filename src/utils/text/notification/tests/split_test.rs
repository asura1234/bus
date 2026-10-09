use super::split_message;

#[test]
fn split_message_preserves_empty_fields_and_requires_colon_space() {
    for message in ["", ": body", "title: ", "title:body", ": ", "plain"] {
        assert_eq!(split_message(message), (message, None));
    }
    assert_eq!(
        split_message("title: body: tail"),
        ("title", Some("body: tail"))
    );
    assert_eq!(split_message(" title:  body "), (" title", Some(" body ")));
}
