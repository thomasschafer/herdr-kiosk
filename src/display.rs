use std::borrow::Cow;

pub fn sanitize(value: &str) -> Cow<'_, str> {
    if !value.chars().any(char::is_control) {
        return Cow::Borrowed(value);
    }

    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            sanitized.extend(character.escape_default());
        } else {
            sanitized.push(character);
        }
    }
    Cow::Owned(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_rendered_as_visible_escapes() {
        assert_eq!(
            sanitize("repo\nname\t\u{1b}").as_ref(),
            "repo\\nname\\t\\u{1b}"
        );
        assert!(matches!(sanitize("plain"), Cow::Borrowed("plain")));
    }
}
