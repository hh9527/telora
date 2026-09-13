//! Names are known at codegen; property evaluation only selects the policy.
use std::collections::BTreeSet;

pub(crate) fn external_names(
    names: impl IntoIterator<Item = String>,
    rename: bool,
) -> Result<Vec<String>, String> {
    let mut seen = BTreeSet::new();
    names
        .into_iter()
        .map(|name| {
            let name = if rename {
                lower_camel_case(&name)
            } else {
                name
            };
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate external codec name {name:?}"));
            }
            Ok(name)
        })
        .collect()
}

fn lower_camel_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut uppercase = false;
    for (index, character) in name.chars().enumerate() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else if index == 0 {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camel_case_preserves_existing_codec_semantics() {
        for (input, expected) in [
            ("Hello_world", "helloWorld"),
            ("URL_value", "uRLValue"),
            ("_first", "First"),
            ("a__b_", "aB"),
            ("État_ß", "étatSS"),
            ("", ""),
        ] {
            assert_eq!(lower_camel_case(input), expected);
        }
        assert!(external_names(["a_b".into(), "aB".into()], true).is_err());
        assert_eq!(
            external_names(["a_b".into(), "aB".into()], false).unwrap(),
            ["a_b", "aB"]
        );
    }
}
